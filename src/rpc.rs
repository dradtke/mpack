use std::collections::HashMap;
use std::old_io::{IoErrorKind, IoResult, Stream};
use std::string::String;
use std::sync::{Arc, Future, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread::Thread;

use super::{Value, read_value, write, write_value};

static MSGID: AtomicUsize = ATOMIC_USIZE_INIT;

pub type RpcResult = Result<Value, Value>;

/// `Client` represents a connection to some server that accepts
/// MessagePack RPC requests.
pub struct Client<R, W> {
    output: W,
    pending: Arc<Mutex<HashMap<u32, Sender<RpcResult>>>>,
}

impl<R, W> Client<R, W> where R: Reader + Send, W: Writer + Send {
    /// Construct a new Client instance from a stream.
    ///
    /// In order for this to work properly, the stream's `.clone()` method must
    /// return a new handle to the shared underlying stream; in other words, it
    /// must be a shallow clone and not deep. `TCPStream` and `UnixStream` are known
    /// to do this, but other types should be aware of this restriction.
    pub fn new_for_stream<S>(stream: S) -> Client<S, S> where S: Stream + Clone + Send {
        Client::new(stream.clone(), stream)
    }

    /// Construct a new Client instance from a reader/writer pair.
    pub fn new(mut reader: R, writer: W) -> Client<R, W> {
        let pending = Arc::new(Mutex::new(HashMap::new()));

        {
            let pending = pending.clone();
            Thread::spawn(move || {
                loop {
                    match read_value(&mut reader) {
                        Ok(value) => assert_eq!(value.int(), 1),
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

    /// Helper for calling methods via RPC.
    fn do_call(&mut self, method: String, params: Vec<Value>) -> IoResult<Receiver<RpcResult>> {
        try!(write_value(&mut self.output, Value::Int8(0))); // for method call

        let msgid = MSGID.fetch_add(1, Ordering::SeqCst) as u32;
        let (responder, listener) = channel();
        self.pending.lock().unwrap().insert(msgid, responder);

        try!(write(&mut self.output, msgid));
        try!(write(&mut self.output, method));
        try!(write_value(&mut self.output, Value::Array(params)));
        try!(self.output.flush());

        Ok(listener)
    }

    /// Call a method via RPC and receive the response as a Future.
    ///
    /// If an `Err` value is returned, it indicates some IO error that occurred while trying
    /// to make the call. An `Ok` value means that the call was successfully made. Calling
    /// `.get()` on its contents will block the thread until a response is received.
    pub fn call(&mut self, method: String, params: Vec<Value>) -> IoResult<Future<RpcResult>> {
        Ok(Future::from_receiver(try!(self.do_call(method, params))))
    }

    /// Call a method via RPC and receive the response in a closure.
    ///
    /// If an `Err` value is returned, it indicates some IO error that occurred while trying
    /// to make the call, and as a result the provided closure will never be invoked. If an
    /// `Ok` value is returned, then a new thread will be kicked off to listen for the response
    /// and pass it to the closure when it's received.
    ///
    /// # Panics
    ///
    /// The new thread could conceivably panic if the Client instance gets cleaned up before
    /// the callback is invoked.
    pub fn call_cb<F: FnOnce(RpcResult) -> () + Send>(&mut self, method: String, params: Vec<Value>, f: F) -> IoResult<()> {
        let listener = try!(self.do_call(method, params));
        Thread::spawn(move || {
            f(listener.recv().unwrap());
        });
        Ok(())
    }

    /// Call a method via RPC and synchronously wait for the response.
    ///
    /// If an `Err` value is returned, it indicates some IO error that occurred while trying
    /// to make the call. An `Ok` value will contain the server's response.
    pub fn call_sync(&mut self, method: String, params: Vec<Value>) -> IoResult<RpcResult> {
        Ok(try!(self.do_call(method, params)).recv().unwrap())
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
    use std::old_io::{ChanReader, ChanWriter, IoErrorKind};
    use std::old_io::timer::sleep;
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
                            sleep(Duration::seconds(2));
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