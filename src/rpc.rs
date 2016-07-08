use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::string::String;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread::{self, JoinHandle};

use super::{Value, ReadError, WriteError, write_value};

static MSGID: AtomicUsize = ATOMIC_USIZE_INIT;

pub type RpcResult = Result<Value, Value>;
type PendingData = Arc<Mutex<HashMap<u32, Sender<RpcResult>>>>;

/// `Client` represents a connection to some server that accepts
/// MessagePack RPC requests.
pub struct Client<R: Read + Send + 'static, W: Write + Send> {
    input: PhantomData<R>, // the reader instance is effectively owned by the main loop
    output: W,
    pending: PendingData,
    main_loop_handle: JoinHandle<()>,
}

impl<R, W> Client<R, W> where R: Read + Send + 'static, W: Write + Send {
    /// Construct a new Client instance from a stream.
    ///
    /// In order for this to work properly, the stream's `.clone()` method must
    /// return a new handle to the shared underlying stream; in other words, it
    /// must be a shallow clone and not deep. `TcpStream` and `UnixStream` are known
    /// to do this, but other types should be aware of this restriction.
    pub fn new_for_stream<S>(stream: S) -> Client<S, S> where S: Read + Write + Clone + Send + 'static {
        Client::new(stream.clone(), stream)
    }

    /// Construct a new Client instance.
    ///
    /// This will spawn a main loop thread which will take ownership of the `Read`
    /// instance, and which will exit when it detects EOF, or when it receives a 0 in lieu
    /// of a MessagePack data type identifier.
    pub fn new(reader: R, writer: W) -> Client<R, W> {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        Client{
            input: PhantomData,
            output: writer,
            pending: pending.clone(),
            main_loop_handle: Client::<R, W>::main_loop(reader, pending),
        }
    }

    /// Call a method via RPC and receive the response as a Receiver.
    ///
    /// If an `Err` value is returned, it indicates some IO error that occurred while trying
    /// to make the call. An `Ok` value means that the call was successfully made. Calling
    /// `.get()` on its contents will block the thread until a response is received.
    pub fn call(&mut self, method: String, params: Vec<Value>) -> Result<Receiver<RpcResult>, WriteError> {
        Ok(try!(self.do_call(method, params)))
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
    pub fn call_cb<F: FnOnce(RpcResult) -> () + Send + 'static>(&mut self, method: String, params: Vec<Value>, f: F) -> Result<(), WriteError> {
        let listener = try!(self.do_call(method, params));
        thread::spawn(move || {
            f(listener.recv().unwrap());
        });
        Ok(())
    }

    /// Call a method via RPC and synchronously wait for the response.
    ///
    /// If an `Err` value is returned, it indicates some IO error that occurred while trying
    /// to make the call. An `Ok` value will contain the server's response.
    pub fn call_sync(&mut self, method: String, params: Vec<Value>) -> Result<RpcResult, WriteError> {
        Ok(try!(self.do_call(method, params)).recv().unwrap())
    }

    /// Runs a scoped thread for dispatching messages received from reader to the appropriate parties.
    fn main_loop(reader: R, pending: PendingData) -> JoinHandle<()> {
        thread::spawn(move || {
            let r = reader;
            let mut reader = super::Reader::new(r);
            loop {
                match reader.read_value() {
                    Ok(value) => match value.array() {
                        Ok(response) => {
                            match response[0].clone().int().unwrap() {
                                1 => {
                                    let msgid = match response[1].clone() {
                                        Value::Int8(i) => i as u32,
                                        Value::Int16(i) => i as u32,
                                        Value::Int32(i) => i as u32,
                                        Value::Uint8(u) => u as u32,
                                        Value::Uint16(u) => u as u32,
                                        Value::Uint32(u) => u as u32,
                                        x => panic!("received unexpected msgid value: {:?}", x),
                                    };
                                    let responder: Sender<RpcResult> = pending.lock().unwrap().remove(&msgid).unwrap();

                                    let error = response[2].clone();
                                    if !error.is_nil() {
                                        responder.send(Err(error)).unwrap();
                                    } else {
                                        let result = response[3].clone();
                                        responder.send(Ok(result)).unwrap();
                                    }
                                },
                                _ => (),
                            }
                        },
                        Err(_) => (),
                    },
                    Err(ReadError::Unrecognized(0)) => break, // kill signal
                    Err(ReadError::NoData) => break, // eof
                    Err(e) => panic!("error: {}", e),
                }
            }
        })
    }

    /// Helper for calling methods via RPC.
    fn do_call(&mut self, method: String, params: Vec<Value>) -> Result<Receiver<RpcResult>, WriteError> {
        let msgid = MSGID.fetch_add(1, Ordering::SeqCst) as u32;
        let (responder, listener) = channel();
        self.pending.lock().unwrap().insert(msgid, responder);

        try!(write_value(&mut self.output, Value::Array(vec![
            Value::Int8(0),
            Value::Uint32(msgid),
            Value::String(method),
            Value::Array(params),
        ])));
        try!(self.output.flush());

        Ok(listener)
    }
}

impl<R, W> Drop for Client<R, W> where R: Read + Send + 'static, W: Write + Send {
    fn drop(&mut self) {
        match self.output.write(&[0]) {
            Ok(_) => (),
            Err(ref e) if e.kind() == io::ErrorKind::BrokenPipe => (), // already closed, no need to kill it
            Err(e) => panic!("{}", e),
        }
    }
}

#[cfg(test)]
mod test {
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::Duration;

    use super::Client;
    use super::super::{Value, ReadError, write, write_value};

    // This test just calls a method "ping" and expects the result "pong".
    #[test] fn simple_test() {
        let (cw, cr) = channel();
        let (sw, sr) = channel();

        thread::spawn(move || {
            let mut reader = ::Reader::new(::test::ChanReader(cr));
            let mut writer = ::test::ChanWriter(sw);

            assert_eq!(reader.read_value().unwrap().int().unwrap(), 0);
            let msgid = reader.read_value().unwrap().uint().unwrap() as u32;
            let method = reader.read_value().unwrap().string().unwrap();
            let _ = reader.read_value().unwrap().array().unwrap(); // params

            match method.as_str() {
                "ping" => {
                    write(&mut writer, 1 as i8).unwrap();
                    write_value(&mut writer, Value::Uint32(msgid)).unwrap();
                    write(&mut writer, ()).unwrap(); // error
                    write(&mut writer, "pong".to_string()).unwrap(); // value
                },
                m => panic!("didn't expect you to call method '{}'", m),
            }
        });

        let mut client = Client::new(::test::ChanReader(sr), ::test::ChanWriter(cw));
        let value = client.call_sync("ping".to_string(), vec![]).unwrap().unwrap();
        assert_eq!(value, Value::String("pong".to_string()));
    }

    /// This method verifies the correctness of mismatched response times by calling
    /// two methods, one slow and one fast, and verifying that the expected value
    /// gets returned to the appropriate caller.
    #[test] fn out_of_order_test() {
        let (cw, cr) = channel();
        let (sw, sr) = channel();

        thread::spawn(move || {
            let mut reader = ::Reader::new(::test::ChanReader(cr));

            loop {
                match reader.read_value() {
                    Ok(value) => assert_eq!(value.int().unwrap(), 0),
                    Err(ReadError::Unrecognized(0)) => break,
                    Err(ReadError::NoData) => break,
                    x => panic!("received unexpected value '{:?}'", x),
                }

                let msgid = reader.read_value().unwrap().uint().unwrap() as u32;
                let method = reader.read_value().unwrap().string().unwrap();
                let params = reader.read_value().unwrap().array().unwrap();
                let sw = sw.clone();

                thread::spawn(move || {
                    let msgid = msgid;
                    let method = method;
                    let mut params = params;
                    let mut writer = ::test::ChanWriter(sw);

                    macro_rules! echo(() => ({
                        write(&mut writer, 1 as i8).unwrap();
                        write_value(&mut writer, Value::Uint32(msgid)).unwrap();
                        write(&mut writer, ()).unwrap(); // error
                        write_value(&mut writer, params.swap_remove(0)).unwrap(); // value
                    }));

                    match method.as_str() {
                        "quick_echo" => {
                            echo!();
                        },
                        "slow_echo" => {
                            thread::sleep(Duration::from_secs(2));
                            echo!();
                        },
                        m => panic!("didn't expect you to call method '{}'", m),
                    }
                });
            }
        });

        let mut client = Client::new(::test::ChanReader(sr), ::test::ChanWriter(cw));

        let hello_future = client.call("slow_echo".to_string(), vec![Value::String("hello".to_string())]).unwrap();
        let world_future = client.call("quick_echo".to_string(), vec![Value::String("world".to_string())]).unwrap();

        assert_eq!(hello_future.recv().unwrap().unwrap(), Value::String("hello".to_string()));
        assert_eq!(world_future.recv().unwrap().unwrap(), Value::String("world".to_string()));
    }

    /// Verify that the value is made available in case of a type error.
    #[test] fn ownership_test() {
        let mut v = Value::Boolean(true);

        v = match v.int() {
            Ok(_) => unreachable!(),
            Err(err) => err.value(),
        };

        match v.bool() {
            Ok(b) => assert_eq!(b, true),
            Err(_) => unreachable!(),
        };
    }
}
