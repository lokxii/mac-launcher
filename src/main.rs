mod backend;
mod frontend;

use backend::*;
use frontend::*;
use std::{
    error::Error,
    io,
    io::Read,
    sync::{mpsc, Arc, Mutex, TryLockError},
    thread,
};
#[macro_use]
extern crate lazy_static;

macro_rules! mutex {
    ($l:ident $op:tt $r:expr) => {
        { *$l.lock().unwrap() $op $r; }
    };
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut app = App::init("Query>")?;

    let cache = Arc::new(Mutex::new(Cache::new()));
    let backend_cache = Arc::clone(&cache);
    let config = Arc::new(Config::from_file(&CONFIG_PATH));
    let backend_config = Arc::clone(&config);
    let (query_tx, query_rx) = mpsc::channel::<String>();
    let (select_tx, select_rx) = mpsc::channel::<LauncherResult>();

    // wait for launching result
    let selection = thread::spawn(move || {
        let config = Arc::clone(&config);
        let magic_cookie = new_magic_cookie().unwrap();
        loop {
            if let Ok(r) = select_rx.recv() {
                if r.select(&*config, &magic_cookie).unwrap() {
                    println!("<Press any key to exit>");
                    io::stdin().lock().read_exact(&mut [0; 1]).unwrap();
                }
                break;
            }
        }
    });

    // backend
    thread::spawn(move || {
        let config = Arc::clone(&backend_config);
        mutex!(backend_cache = Cache::init(&config));

        while let Ok(s) = query_rx.recv() {
            if !s.is_empty() {
                let config = Arc::clone(&config);
                let backend_cache = Arc::clone(&backend_cache);
                thread::spawn(move || {
                    let new_cache = {
                        let inner = backend_cache.lock().unwrap().clone();
                        Query::from(s.as_str()).parse(&config, inner).unwrap()
                    };
                    *backend_cache.lock().unwrap() = new_cache;
                });
            }
        }
    });

    // UI
    let mut results: Arc<Vec<LauncherResult>> = Arc::new(vec![]);
    loop {
        let mut index = None;
        query_tx.send(app.get_query()).unwrap();
        results = match cache.try_lock() {
            Ok(r) => r.get_results(&app.get_query()).unwrap_or(results),
            Err(r) => {
                if let TryLockError::WouldBlock = r {
                    results
                } else {
                    panic!("{:?}", r);
                }
            }
        };
        if app.update(&results)?.wait_input(&mut index).unwrap() {
            app.exit();
            if let Some(i) = index {
                select_tx.send(results[i].clone())?;
                selection.join().unwrap();
            }
            break;
        }
    }
    return Ok::<(), Box<dyn Error>>(());
}
