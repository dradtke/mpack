#![feature(io)]
#![allow(dead_code)]

use std::io::{Read, Write};
use std::thread;

struct Client<R, W> where R: Read, W: Write {
    r: R,
    w: W,
}

impl<R, W> Client<R, W> where R: Read + Send, W: Write + Send {
    pub fn new(r: R, w: W) -> Client<R, W> {
        let mut client = Client{ r: r, w: w };
        client.listen();
        client
    }

    fn listen(&mut self) {
        thread::scoped(|| {
            let mut buf = [0 as u8; 1024];
            loop {
                match self.r.read(&mut buf) {
                    Ok(read) => println!("read {} bytes", read),
                    Err(..) => println!("fuck"),
                }
            }
        });
    }
}

fn main() {}
