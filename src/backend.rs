use dns_lookup::lookup_host;
use filemagic::{flags::Flags, FileMagicError, Magic};
use fuse_rust::Fuse;
// use regex::Regex;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use lazy_static;
use rayon::prelude::*;
use serde_derive::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::HashMap,
    env,
    error::Error,
    fs, io,
    path::Path,
    process::{Child, Command},
};
use url::Url;

// TODO: use config file

lazy_static! {
    pub static ref CONFIG_PATH: String =
        env::var("HOME").unwrap() + "/.config/launcher/launcher.toml";
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
            ],
            editor: "hx".to_string(),
            results_len: 20,
            fuzzy_engine: "skim".to_string(),
        }
    }

    pub fn from_file(path: &str) -> Config {
        if let Ok(s) = fs::read_to_string(path) {
            toml::from_str(&s).unwrap_or(Config::default())
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
                return Ok(run_command(cmd, param)?);
            }
            Self::Url(url) => {
                spawn_process(&format!("open '{}'", url))?.wait()?;
            }
            Self::App(path) => {
                spawn_process(&format!("open '{}'", path))?.wait()?;
            }
            Self::Bin(path) => {
                spawn_process(&format!("{}", path))?.wait()?;
                return Ok(true);
            }
            Self::File(path) => {
                let magic = magic_cookie
                    .file(path)
                    .expect(&format!("failed to check magic of file `{}`", path));
                // is text file?
                if ["text", "json", "csv"]
                    .iter()
                    .any(|s| magic.to_lowercase().contains(s))
                {
                    spawn_process(&format!("{} '{}'", config.editor, path))?.wait()?;
                } else {
                    spawn_process(&format!("open '{}'", path))?.wait()?;
                }
            }
        };
        return Ok(false);
    }

    fn prerun_command(self, cache: &mut Cache) -> io::Result<Vec<LauncherResult>> {
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
            LauncherResult::Command(cmd, param) => format!(":{} {}", cmd, param),
            LauncherResult::Url(url) => format!("Url: {}", url),
            LauncherResult::App(app) => format!("App: {}", app),
            LauncherResult::Bin(bin) => format!("Bin: {}", bin),
            LauncherResult::File(file) => format!("File: {}", file),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FileEntryType {
    App,
    Bin,
    File,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    file_type: FileEntryType,
    full_path: String,
    name: String,
}

#[derive(Debug)]
pub struct Cache {
    file_entries: Vec<FileEntry>,
    search_results: HashMap<String, Vec<LauncherResult>>,
}

macro_rules! into_string {
    ($expr:expr) => {
        $expr.to_str().unwrap().to_string()
    };
}

impl Cache {
    pub fn new() -> Cache {
        return Cache {
            file_entries: vec![],
            search_results: HashMap::new(),
        };
    }

    fn add_dir<T>(&mut self, locations: T, r#type: FileEntryType)
    where
        T: IntoIterator,
        <T as IntoIterator>::Item: AsRef<Path>,
    {
        for location in locations {
            if let Ok(dir) = fs::read_dir(location) {
                for path in dir {
                    let path = path.unwrap();

                    let name = into_string!(path.file_name());
                    let name = if let FileEntryType::App = r#type {
                        if name.starts_with('.') {
                            continue;
                        }
                        if let Some((name, _)) = name.split_once('.') {
                            name.to_string()
                        } else {
                            name
                        }
                    } else {
                        name
                    };
                    self.file_entries.push(FileEntry {
                        file_type: r#type,
                        full_path: into_string!(path.path()),
                        name,
                    })
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
        return cache;
    }

    fn get_results(&self, query: &str) -> Option<Vec<LauncherResult>> {
        if self.search_results.contains_key(query) {
            return Some(self.search_results[query].clone());
        } else {
            None
        }
    }

    fn add_results(&mut self, query: &str, results: Vec<LauncherResult>) {
        self.search_results.insert(query.to_string(), results);
    }

    fn search(&self, query: &str, kind: &str, config: &Config) -> Vec<LauncherResult> {
        let mut results: Vec<LauncherResult> = vec![];

        let fuzzy_search_results: Vec<&FileEntry> = match kind {
            "skim" => {
                let skim = SkimMatcherV2::default();
                let mut fuzzy_search_results = self
                    .file_entries
                    .par_iter()
                    .filter_map(|x| Some((skim.fuzzy_match(&x.name, query)?, x)))
                    .collect::<Vec<(i64, &FileEntry)>>();
                fuzzy_search_results.sort_unstable_by_key(|e| Reverse(e.0));
                fuzzy_search_results.iter().map(|e| e.1).collect()
            }

            "fuse" => {
                let mut fuse = Fuse::default();
                fuse.threshold = 0.4;

                // TODO: use BTreeMap?
                let pattern = fuse.create_pattern(query);
                let mut fuzzy_search_results = self
                    .file_entries
                    .par_iter()
                    .filter_map(|x| {
                        if query.len() <= x.name.len() {
                            Some((fuse.search(pattern.as_ref(), &x.name)?, x))
                        } else {
                            None
                        }
                    })
                    .map(|e| {
                        let coverage = (e.1.name.len()
                            - e.0
                                .ranges
                                .iter()
                                .map(|range| range.end - range.start)
                                .sum::<usize>()) as f64
                            / e.1.name.len() as f64
                            * 1000.0;
                        ((e.0.score * 1000.0) as i64, coverage as i64, e.1)
                    })
                    .collect::<Vec<(i64, i64, &FileEntry)>>();
                fuzzy_search_results.sort_unstable_by_key(|e| (e.0, e.1));
                fuzzy_search_results.iter().map(|e| e.2).collect()
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
        results.extend(
            fuzzy_search_results[0..end_index]
                .iter()
                .map(|r| match r.file_type {
                    FileEntryType::App => LauncherResult::App(r.full_path.clone()),
                    FileEntryType::Bin => LauncherResult::Bin(r.full_path.clone()),
                    FileEntryType::File => LauncherResult::File(r.full_path.clone()),
                }),
        );
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

    pub fn parse<'a>(
        &self,
        config: &Config,
        cache: &'a mut Cache,
    ) -> io::Result<Vec<LauncherResult>> {
        let query = self.0.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }

        if let Some(results) = cache.get_results(query) {
            return Ok(results);
        }
        let mut results: Vec<LauncherResult> = vec![];

        // History
        // TODO: save search queries, exec commands

        // Command
        if query.starts_with(':') {
            if let Some((cmd, param)) = query[1..].trim().split_once(' ') {
                results.extend(
                    LauncherResult::Command(cmd.trim().to_string(), param.trim().to_string())
                        .prerun_command(cache)?,
                );
            } else {
                results.extend(
                    LauncherResult::Command(query[1..].trim().to_string(), String::new())
                        .prerun_command(cache)?,
                );
            }
            cache.add_results(query, results.clone());
            return Ok(results);
        }

        // fuzzy search app / bin / opened files
        // only search of query.len() < 15
        if query.len() < 15 {
            results.extend(cache.search(query, &config.fuzzy_engine, config));
        }

        // Url
        if let Ok(_) = lookup_host(query) {
            results.push(LauncherResult::Url(Query::fix_url(query)));
            cache.add_results(query, results.clone());
        }

        results.push(LauncherResult::Command(
            "search".to_string(),
            query.to_string(),
        ));

        cache.add_results(query, results.clone());
        return Ok(results);
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

fn spawn_process(s: &str) -> io::Result<Child> {
    return Command::new("bash").arg("-c").arg(s).spawn();
}

fn run_command(cmd: &str, param: &str) -> Result<bool, Box<dyn Error>> {
    match cmd {
        "search" => {
            let mut url = Url::parse("https://www.google.com/search?")?;
            url.query_pairs_mut().append_pair("q", param);
            spawn_process(&format!("open '{}'", url.as_str()))?.wait()?;

            Ok(false)
        }
        "exec" => {
            spawn_process(&format!("{}", param))?.wait()?;
            Ok(true)
        }
        &_ => Ok(false),
    }
}
