use criterion::{black_box, criterion_group, criterion_main, Criterion};
use launcher::backend::*;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

fn searching(c: &mut Criterion) {
    let queries = [
        "Whatsapp",
        "Telegram",
        "VPN",
        "App Store",
        "music",
        "settin",
        "calculator",
        "safa",
        "fish",
        "ssh",
        "acitivi moni",
        "alsdkj;sl",
        " 38y lksdjhf o8",
        "P* LAS DLK#I",
        "(@ OIJDNF O#(P(UQ {)( HIL*EYP IXZKLJHkcjhdflkjshlfkysi8h )})))",
        ":search",
        ":exec",
        ":update",
        ":ioay4 kjhdfgio 48 lkjdfhs",
        ":p9383 AUHW#*(Y LIHFP#*(YUPOA*U))",
    ];
    let config = Arc::new(Config::from_file(&CONFIG_PATH));
    let cache = Arc::new(Mutex::new(Cache::init(&config)));
    c.bench_function("running backend with 20 queries multithreaded", |b| {
        b.iter(|| {
            for query in queries {
                for i in 0..query.len() {
                    let cache = Arc::clone(&cache);
                    let config = Arc::clone(&config);
                    thread::spawn(move || {
                        let query = black_box(&query[0..i]);
                        let mut new_cache = {
                            let inner = cache.lock().unwrap().clone();
                            Query::from(query).parse(&config, inner).unwrap()
                        };
                        let mut inner = cache.lock().unwrap();
                        for f in new_cache.file_entries {
                            inner.file_entries.insert(f);
                        }
                        if let Some(r) = new_cache.search_results.remove(query) {
                            inner.search_results.insert(query.to_string(), r);
                        }
                    });
                    sleep(Duration::from_millis(1000 / 90));
                }
            }
        })
    });
}

criterion_group! {
    name = benches;
    // This can be any expression that returns a `Criterion` object.
    config = Criterion::default().significance_level(0.1).sample_size(10);
    targets = searching
}
criterion_main!(benches);
