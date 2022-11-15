mod backend;
mod frontend;

use backend::*;
use frontend::*;
use std::error::Error;
use std::io::Read;
use std::{io, thread};

fn main() -> Result<(), Box<dyn Error>> {
    let mut app = App::init("Query>")?;
    let config = Config::default();
    let mut cache = Cache::init(&config);
    let magic_cookie = new_magic_cookie()?;
    loop {
        let mut index = None;
        let results = Query::from(&app.get_query())
            .parse(&config, &mut cache)
            .unwrap();
        if app.update(&results)?.wait_input(&mut index).unwrap() {
            app.exit();
            if let Some(i) = index {
                if results[i].select(&config, &magic_cookie)? {
                    println!("<Press any key to exit>");
                    io::stdin().read_exact(&mut [0; 1])?;
                }
            }
            break;
        }
    }
    return Ok::<(), Box<dyn Error>>(());
}
