use std::collections::HashMap;
use std::io::{IoErrorKind, IoResult};
use std::string::String;
use std::sync::{Arc, Future, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::sync::mpsc::{channel, Sender};
use std::thread::Thread;

use super::{Value, read_value, write, write_value};

static MSGID: AtomicUsize = ATOMIC_USIZE_INIT;

type RpcResult = Result<Value, Value>;

pub struct Client<R, W> {
    output: W,
    pending: Arc<Mutex<HashMap<u32, Sender<RpcResult>>>>,
}

impl<R, W> Client<R, W> where R: Reader + Send, W: Writer + Send {
    pub fn new(mut reader: R, writer: W) -> Client<R, W> {
        let pending = Arc::new(Mutex::new(HashMap::new()));

        {
            let pending = pending.clone();
            Thread::spawn(move || {
                loop {
                    match read_value(&mut reader) {
                        Ok(Value::Int8(1)) => (), // response
                        Ok(value) => panic!("unexpected value; expected 1i8, got {:?}", value),
                        Err(super::ReadError::Unrecognized(0)) => break, // kill signal
                        Err(super::ReadError::Io(ref e)) if e.kind == IoErrorKind::EndOfFile => break,
                        Err(e) => panic!("error: {}", e),
                    }

                    match read_value(&mut reader) {
                        Ok(Value::Uint32(msgid)) => {
                            let responder: Sender<RpcResult> = pending.lock().unwrap().remove(&msgid).unwrap();

                            let err = read_value(&mut reader).unwrap();
                            let res = read_value(&mut reader).unwrap();

                            responder.send(if err == Value::Nil { Ok(res) } else { Err(err) }).unwrap();
                        },
                        Ok(value) => panic!("received value, but it wasn't a u32: {:?}", value),
                        Err(e) => panic!("failed to receive value: {:?}", e),
                    }
                }
            });
        }

        Client { output: writer, pending: pending }
    }

    pub fn call(&mut self, method: String, params: Vec<Value>) -> IoResult<Future<RpcResult>> {
        try!(write_value(&mut self.output, Value::Int8(0))); // for method call

        let msgid = MSGID.fetch_add(1, Ordering::SeqCst) as u32;
        let (responder, listener) = channel();
        self.pending.lock().unwrap().insert(msgid, responder);

        try!(write(&mut self.output, msgid));
        try!(write(&mut self.output, method));
        try!(write_value(&mut self.output, Value::Array(params)));
        try!(self.output.flush());

        Ok(Future::from_receiver(listener))
    }
}

#[unsafe_destructor]
impl<R, W> Drop for Client<R, W> where W: Writer {
    fn drop(&mut self) {
        match self.output.write_u8(0) {
            Ok(_) => (),
            Err(ref e) if e.kind == IoErrorKind::BrokenPipe => (), // already closed, no need to kill it
            Err(e) => panic!("{}", e),
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::{ChanReader, ChanWriter, IoErrorKind};
    use std::io::timer::sleep;
    use std::sync::mpsc::{channel};
    use std::thread::Thread;
    use std::time::duration::Duration;
    use super::Client;
    use super::super::{Value, ReadError, read_value, write, write_value};

    /// This test just calls a method "ping" and expects the result "pong".
    #[test] fn simple_test() {
        let (cw, cr) = channel();
        let (sw, sr) = channel();

        let guard = Thread::scoped(move || {
            let mut cr = ChanReader::new(cr);
            let mut sw = ChanWriter::new(sw);

            assert_eq!(read_value(&mut cr).unwrap().int(), 0);
            let msgid = read_value(&mut cr).unwrap().uint() as u32;
            let method = read_value(&mut cr).unwrap().string();
            let _ = read_value(&mut cr).unwrap().array(); // params

            match method.as_slice() {
                "ping" => {
                    write(&mut sw, 1 as i8).unwrap();
                    write_value(&mut sw, Value::Uint32(msgid)).unwrap();
                    write(&mut sw, ()).unwrap(); // error
                    write(&mut sw, "pong".to_string()).unwrap(); // value
                },
                m => panic!("didn't expect you to call method '{}'", m),
            }
        });

        let mut client = Client::new(ChanReader::new(sr), ChanWriter::new(cw));
        let mut value_future = client.call("ping".to_string(), vec![]).unwrap();
        let value = value_future.get().unwrap();
        assert_eq!(value, Value::String("pong".to_string()));

        assert!(guard.join().is_ok());
    }

    /// This method verifies the correctness of mismatched response times by calling
    /// two methods, one slow and one fast, and verifying that the expected value
    /// gets returned to the appropriate caller.
    #[test] fn out_of_order_test() {
        let (cw, cr) = channel();
        let (sw, sr) = channel();

        Thread::spawn(move || {
            let mut cr = ChanReader::new(cr);
            let sw = ChanWriter::new(sw);

            loop {
                match read_value(&mut cr) {
                    Ok(value) => assert_eq!(value.int(), 0),
                    Err(ReadError::Io(ref e)) if e.kind == IoErrorKind::EndOfFile => break,
                    Err(ReadError::Unrecognized(0)) => break,
                    x => panic!("received unexpected value '{:?}'", x),
                }

                let msgid = read_value(&mut cr).unwrap().uint() as u32;
                let method = read_value(&mut cr).unwrap().string();
                let params = read_value(&mut cr).unwrap().array();
                let mut sw = sw.clone();

                Thread::spawn(move || {
                    let msgid = msgid;
                    let method = method;
                    let mut params = params;

                    macro_rules! echo(() => ({
                        write(&mut sw, 1 as i8).unwrap();
                        write_value(&mut sw, Value::Uint32(msgid)).unwrap();
                        write(&mut sw, ()).unwrap(); // error
                        write_value(&mut sw, params.swap_remove(0)).unwrap(); // value
                    }));

                    match method.as_slice() {
                        "quick_echo" => {
                            echo!();
                        },
                        "slow_echo" => {
                            sleep(Duration::seconds(3));
                            echo!();
                        },
                        m => panic!("didn't expect you to call method '{}'", m),
                    }
                });
            }
        });

        let mut client = Client::new(ChanReader::new(sr), ChanWriter::new(cw));

        let mut hello_future = client.call("slow_echo".to_string(), vec![Value::String("hello".to_string())]).unwrap();
        let mut world_future = client.call("quick_echo".to_string(), vec![Value::String("world".to_string())]).unwrap();

        assert_eq!(hello_future.get().unwrap(), Value::String("hello".to_string()));
        assert_eq!(world_future.get().unwrap(), Value::String("world".to_string()));
    }
}
