// Quick test harness: send an IDE script and print the response.
// Usage: cargo run --example ide_test -- 'Print "hello"'

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn main() {
    let script = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: cargo run --example ide_test -- '<ide script>'");
        std::process::exit(1);
    });

    let tag = format!("test_{}", std::process::id());
    let proto = serde_json::json!({"protocol": 2});
    let request = serde_json::json!({"tag": tag, "script": script});

    let mut payload = Vec::new();
    payload.extend_from_slice(proto.to_string().as_bytes());
    payload.push(0);
    payload.extend_from_slice(request.to_string().as_bytes());
    payload.push(0);

    let socket_path = if std::path::Path::new("/tmp/XojoIDE").exists() {
        "/tmp/XojoIDE"
    } else {
        "/private/tmp/XojoIDE"
    };

    let mut stream =
        UnixStream::connect(socket_path).expect("Failed to connect to Xojo IDE socket");
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.write_all(&payload).expect("Failed to send");
    stream.flush().ok();

    let mut buf = vec![0u8; 65536];
    let mut total = 0;
    loop {
        match stream.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                // Check if we have a complete response (ends with NUL or contains our tag).
                let data = &buf[..total];
                for chunk in data.split(|&b| b == 0) {
                    if chunk.is_empty() {
                        continue;
                    }
                    if let Ok(s) = std::str::from_utf8(chunk)
                        && s.contains(&tag)
                    {
                        println!("{s}");
                        return;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => {
                eprintln!("Read error: {e}");
                break;
            }
        }
    }
    eprintln!("No matching response received");
}
