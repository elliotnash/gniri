// SPDX-License-Identifier: GPL-3.0-or-later

//! Takes pictures of a part of an output with the capture backend, for
//! development, and says how long each took:
//!
//! ```text
//! cargo run -p gniri-services --example capture -- eDP-1 0 0 400 300 [FILE]
//! ```
//!
//! The rectangle is in logical pixels. With FILE the last picture's
//! pixels are written there as they are, four bytes each.

use std::time::{Duration, Instant};

use gniri_services::capture::{Capturer, Rect};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let numbers: Vec<i32> = arguments
        .iter()
        .skip(1)
        .take(4)
        .filter_map(|argument| argument.parse().ok())
        .collect();
    let (Some(output), &[x, y, width, height]) = (arguments.first(), &numbers[..]) else {
        eprintln!("usage: capture OUTPUT X Y WIDTH HEIGHT [FILE]");
        std::process::exit(2);
    };
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    let capturer = Capturer::spawn();
    // The backend learns of the outputs on its own thread.
    std::thread::sleep(Duration::from_millis(200));
    let mut last = None;
    for _ in 0..5 {
        let asked = Instant::now();
        let answer = futures_lite::future::block_on(capturer.capture(output, rect).recv());
        let took = asked.elapsed().as_secs_f64() * 1000.0;
        match answer {
            Ok(Some(picture)) => {
                println!(
                    "{}x{} stride {} {:?} in {took:.1} ms",
                    picture.width, picture.height, picture.stride, picture.format
                );
                last = Some(picture);
            }
            Ok(None) => println!("no picture, in {took:.1} ms"),
            Err(_) => println!("the backend went away"),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let (Some(file), Some(picture)) = (arguments.get(5), last)
        && let Err(error) = std::fs::write(file, &picture.pixels)
    {
        eprintln!("cannot write {file}: {error}");
    }
}
