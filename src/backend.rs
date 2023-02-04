use dns_lookup::lookup_host;
use filemagic::{flags::Flags, FileMagicError, Magic};
use fuse_rust::Fuse;
// use regex::Regex;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use rayon::prelude::*;
use serde_derive::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    env,
    error::Error,
    fs,
    hash::{Hash, Hasher},
    io,
    path::Path,
    process::Command,
    os::unix::process::CommandExt,
    sync::Arc,
    thread,
};
use url::Url;

// TODO: use config file

lazy_static! {
    pub static ref HOME_PATH: String = env::var("HOME").unwrap();
    pub static ref CONFIG_PATH: String = HOME_PATH.to_string() + "/.config/launcher/launcher.toml";
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    app_locations: Vec<String>,
    editor: String,       // path to binary
    results_len: usize,   // show how many results
    fuzzy_engine: String, // 'fuse' or 'skim'. Use skim if fuse is too slow
}

impl Config {
    pub fn default() -> Config {
        Config {
            app_locations: vec![
                "/Applications".to_string(),
                "/System/Applications".to_string(),
                "/System/Applications/Utilities".to_string(),
                "/System/Library/CoreServices/Applications".to_string(),
            ],
            editor: "hx".to_string(),
            results_len: 20,
            fuzzy_engine: "skim".to_string(),
        }
    }

    pub fn from_file(path: &str) -> Config {
        if let Ok(s) = fs::read_to_string(path) {
            toml::from_str(&s).unwrap_or_else(|_| Config::default())
        } else {
            Config::default()
        }
    }

    pub fn write_to_file(&self, path: &str) -> Result<(), Box<dyn Error>> {
        let path = Path::new(path);
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        };
        fs::write(path, toml::to_string(self)?.as_bytes())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum LauncherResult {
    Command(String, String), // command description?
    Url(String),             // opens browser
    App(String),
    Bin(String),
    File(String),
    // WebSearch(String), // Retrieve google results
}

impl LauncherResult {
    pub fn select(&self, config: &Config, magic_cookie: &Magic) -> Result<bool, Box<dyn Error>> {
        match self {
            Self::Command(cmd, param) => {
                return run_command(cmd, param);
            }
            Self::Url(url) => {
                exec_process(&format!("open '{}'", url));
            }
            Self::App(path) => {
                exec_process(&format!("open '{}'", path));
            }
            Self::Bin(path) => {
                exec_process(&format!("{}", path));
                return Ok(true);
            }
            Self::File(path) => {
                let magic = magic_cookie
                    .file(path)
                    .unwrap_or_else(|_| panic!("failed to check magic of file `{}`", path));
                // is text file?
                if ["text", "json", "csv"]
                    .iter()
                    .any(|s| magic.to_lowercase().contains(s))
                {
                    exec_process(&format!("{} '{}'", config.editor, path));
                } else {
                    exec_process(&format!("open '{}'", path));
                }
            }
        };
        return Ok(false);
    }

    fn prerun_command(self, cache: &Cache) -> io::Result<Vec<LauncherResult>> {
        if let LauncherResult::Command(cmd, param) = &self {
            match cmd.as_str() {
                "find" => {
                    // BFS file directory
                    Ok(vec![])
                }
                "config" => {
                    // open config file
                    Ok(vec![LauncherResult::File(CONFIG_PATH.clone())])
                }
                _ => Ok(vec![self]),
            }
        } else {
            return Ok(vec![self]);
        }
    }

    pub fn get_string(&self) -> String {
        match self {
            LauncherResult::Command(cmd, param) => format!("Cmd  | :{} {}", cmd, param),
            LauncherResult::Url(url) => format!("Url  | {}", url),
            LauncherResult::App(app) => format!("App  | {}", app),
            LauncherResult::Bin(bin) => format!("Bin  | {}", bin),
            LauncherResult::File(file) => format!("File | {}", file),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEntryType {
    App,
    Bin,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    file_type: FileEntryType,
    full_path: String,
    name: String,
}

impl Hash for FileEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.full_path.hash(state);
    }
}

#[derive(Debug, Clone)]
pub struct Cache {
    pub file_entries: HashSet<Arc<FileEntry>>,
    pub search_results: HashMap<String, Arc<Vec<LauncherResult>>>,
}

macro_rules! into_string {
    ($expr:expr) => {
        $expr.to_str().unwrap().to_string()
    };
}

impl Cache {
    pub fn new() -> Cache {
        return Cache {
            file_entries: HashSet::new(),
            search_results: HashMap::new(),
        };
    }

    fn parent_entry<P>(location: P) -> FileEntry
    where
        P: AsRef<Path>,
    {
        let path = Path::new(location.as_ref());
        let parent = path.parent().unwrap_or(Path::new(""));
        let full_path = into_string!(parent);
        let full_path = if full_path != "/" {
            full_path + "/"
        } else {
            full_path
        };
        let name = if let Some(name) = parent.file_name() {
            into_string!(name) + "/"
        } else {
            String::from("/")
        };
        FileEntry {
            file_type: FileEntryType::File,
            full_path,
            name,
        }
    }

    fn add_dir<T>(&mut self, locations: T, r#type: FileEntryType)
    where
        T: IntoIterator,
        <T as IntoIterator>::Item: AsRef<Path>,
    {
        for location in locations {
            if let Ok(dir) = fs::read_dir(&location) {
                // Add the director it self. Mark it as `file`
                {
                    let entry = Cache::parent_entry(&location);
                    self.file_entries.insert(Arc::new(entry));
                }
                // Then the directory content
                for path in dir {
                    let path = path.unwrap();

                    let name = into_string!(path.file_name());
                    self.file_entries.insert(Arc::new(FileEntry {
                        file_type: r#type,
                        full_path: into_string!(path.path()),
                        name,
                    }));
                }
            }
        }
    }

    pub fn init(config: &Config) -> Cache {
        let mut cache = Cache::new();
        cache.add_dir(&config.app_locations, FileEntryType::App);
        cache.add_dir(
            &env::var("PATH").unwrap().split(':').collect::<Vec<&str>>(),
            FileEntryType::Bin,
        );
        cache.add_dir(&[HOME_PATH.to_string()], FileEntryType::File);
        return cache;
    }

    pub fn get_results(&self, query: &str) -> Option<Arc<Vec<LauncherResult>>> {
        if self.search_results.contains_key(query) {
            return Some(self.search_results[query].clone());
        } else {
            None
        }
    }

    pub fn add_results(&mut self, query: &str, results: Vec<LauncherResult>) {
        self.search_results
            .insert(query.to_string(), Arc::new(results));
    }

    fn search(&self, query: &str, kind: &str, config: &Config) -> Vec<LauncherResult> {
        let mut results: Vec<LauncherResult> = vec![];

        let fuzzy_search_results: Vec<Arc<FileEntry>> = match kind {
            "skim" => {
                let skim = SkimMatcherV2::default();
                let mut fuzzy_search_results = self
                    .file_entries
                    .par_iter()
                    .filter_map(|x| {
                        let (score, indices) = skim.fuzzy_indices(&x.name, query)?;
                        let coverage = indices.len() * 1024 / x.name.len();
                        Some((score, coverage, Arc::clone(&x)))
                    })
                    .collect::<Vec<(i64, usize, Arc<FileEntry>)>>();
                fuzzy_search_results.sort_unstable_by_key(|e| (Reverse(e.0), Reverse(e.1)));
                fuzzy_search_results
                    .iter()
                    .map(|e| Arc::clone(&e.2))
                    .collect()
            }

            "fuse" => {
                let fuse = Fuse {
                    threshold: 0.4,
                    ..Default::default()
                };

                // TODO: use BTreeMap?
                let pattern = fuse.create_pattern(query);
                let mut fuzzy_search_results = self
                    .file_entries
                    .par_iter()
                    .filter_map(|x| {
                        if query.len() <= x.name.len() {
                            let result = fuse.search(pattern.as_ref(), &x.name)?;
                            let coverage = (x.name.len() * 512
                                - result
                                    .ranges
                                    .iter()
                                    .map(|range| range.end - range.start)
                                    .sum::<usize>()
                                    * 512)
                                / x.name.len();
                            Some(((result.score * 512.0) as i64, coverage, Arc::clone(x)))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<(i64, usize, Arc<FileEntry>)>>();
                fuzzy_search_results.sort_unstable_by_key(|e| (e.0, e.1));
                fuzzy_search_results
                    .iter()
                    .map(|e| Arc::clone(&e.2))
                    .collect()
            }
            _ => {
                panic!("Invalid kind");
            }
        };

        let end_index = if fuzzy_search_results.len() < config.results_len {
            fuzzy_search_results.len()
        } else {
            config.results_len
        };
        // FIXME: does it change order?
        results.par_extend(fuzzy_search_results[0..end_index].par_iter().map(
            |r| match r.file_type {
                FileEntryType::App => LauncherResult::App(r.full_path.clone()),
                FileEntryType::Bin => LauncherResult::Bin(r.full_path.clone()),
                FileEntryType::File => LauncherResult::File(r.full_path.clone()),
            },
        ));
        return results;
    }
}

pub struct Query(String);

impl Query {
    pub fn new() -> Query {
        Query(String::new())
    }

    pub fn from(s: &str) -> Query {
        Query(s.to_string())
    }

    // return new Cache entries only
    pub fn parse(&self, config: &Config, cache: Cache) -> io::Result<Cache> {
        let mut delta = Cache::new();

        let query = self.0.trim();
        if query.is_empty() {
            return Ok(delta);
        }

        if cache.get_results(query).is_some() {
            return Ok(delta);
        }
        let mut results: Vec<LauncherResult> = vec![];

        // History
        // TODO: save search queries, exec commands

        // Command
        if let Some(stripped) = query.strip_prefix(':') {
            if let Some((cmd, param)) = stripped.trim().split_once(' ') {
                results.extend(
                    LauncherResult::Command(cmd.trim().to_string(), param.trim().to_string())
                        .prerun_command(&cache)?,
                );
            } else {
                results.extend(
                    LauncherResult::Command(query[1..].trim().to_string(), String::new())
                        .prerun_command(&cache)?,
                );
            }
            delta.add_results(query, results);
            return Ok(delta);
        }

        // Url
        let query_clone = query.to_string();
        let lookup_host_thread = thread::spawn(move || lookup_host(&query_clone));

        // fuzzy search app / bin / opened files
        // only search of query.len() < 15
        if query.len() < 15 {
            results.extend(cache.search(query, &config.fuzzy_engine, config));
        }

        // File path
        if Path::new(query).exists() {
            results.push(LauncherResult::File(query.to_string()));
        }
        // Relative to $HOME directory
        let relative = HOME_PATH.clone() + "/" + query;
        if Path::new(&relative).exists() {
            results.push(LauncherResult::File(relative));
        }

        if let Ok(Ok(_)) = lookup_host_thread.join() {
            results.push(LauncherResult::Url(Self::fix_url(query)));
        }

        results.push(LauncherResult::Command(
            "search".to_string(),
            query.to_string(),
        ));

        delta.add_results(query, results);
        return Ok(delta);
    }

    // TODO: more rules
    fn fix_url(url: &str) -> String {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return String::from("http://") + url;
        }
        return url.to_string();
    }
}

pub fn new_magic_cookie() -> Result<Magic, FileMagicError> {
    let magic_flags = vec![
        Flags::NO_CHECK_APPTYPE,
        Flags::NO_CHECK_COMPRESS,
        Flags::NO_CHECK_ELF,
        Flags::NO_CHECK_ENCODING,
        Flags::NO_CHECK_TOKENS,
    ];
    let mut magic_open_flag = Flags::empty();
    for flag in magic_flags {
        magic_open_flag.insert(flag);
    }
    let cookie = Magic::open(magic_open_flag)?;
    cookie.load::<String>(&[])?;
    return Ok(cookie);
}

fn exec_process(s: &str) -> io::Error {
    return Command::new("bash").arg("-l").arg("-c").arg(s).exec();
}

fn run_command(cmd: &str, param: &str) -> Result<bool, Box<dyn Error>> {
    match cmd {
        "search" => {
            let mut url = Url::parse("https://www.google.com/search?")?;
            url.query_pairs_mut().append_pair("q", param);
            exec_process(&format!("open '{}'", url.as_str()));

            Ok(false)
        }
        "exec" => {
            exec_process(param);
            Ok(true)
        }
        "update" => {
            exec_process(&format!(
                "cd {} && git pull && cargo build --release",
                env!("CARGO_MANIFEST_DIR")
            ));
            Ok(true)
        }
        &_ => Ok(false),
    }
}
