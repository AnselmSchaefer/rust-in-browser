//! Exercises the browser's WASI host: arguments, environment, files, directories, threads, sleep.
//! `wasi-smoke panic` ends with a panic in a spawned thread instead.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("args: {args:?}");
    println!("HELLO={:?}", std::env::var("HELLO"));
    std::fs::create_dir_all("/tmp/smoke/a/b").unwrap();
    std::fs::write("/tmp/smoke/a/b/hello.txt", "hello file").unwrap();
    let mut f = std::fs::OpenOptions::new().append(true).open("/tmp/smoke/a/b/hello.txt").unwrap();
    f.write_all(b", appended").unwrap();
    println!("read back: {}", std::fs::read_to_string("/tmp/smoke/a/b/hello.txt").unwrap());
    let names: Vec<_> = std::fs::read_dir("/tmp/smoke/a/b").unwrap().map(|e| e.unwrap().file_name()).collect();
    println!("dir: {names:?}");
    std::fs::rename("/tmp/smoke/a/b/hello.txt", "/tmp/smoke/moved.txt").unwrap();
    println!("moved exists: {}", std::path::Path::new("/tmp/smoke/moved.txt").exists());
    println!("missing: {:?}", std::fs::read("/nope").map_err(|e| e.kind()));
    let t = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(50));
    println!("slept {} ms", t.elapsed().as_millis());
    let handles: Vec<_> = (0..4)
        .map(|i| {
            std::thread::spawn(move || {
                if i == 2 && std::env::args().any(|a| a == "panic") {
                    panic!("thread {i} panics");
                }
                (0..1_000_000u64).map(|x| x * i).sum::<u64>()
            })
        })
        .collect();
    let sums: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    println!("thread sums: {sums:?}");
    let big = vec![7u8; 300 << 20];
    std::fs::write("/tmp/smoke/big.bin", &big).unwrap();
    println!("big file: {} bytes", std::fs::metadata("/tmp/smoke/big.bin").unwrap().len());
    eprintln!("done (stderr)");
}
