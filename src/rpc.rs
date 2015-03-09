use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::string::String;
use std::sync::{Arc, Future, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread::{self, JoinGuard};

use super::{Value, ReadError, WriteError, write, write_value};

static MSGID: AtomicUsize = ATOMIC_USIZE_INIT;

pub type RpcResult = Result<Value, Value>;
type PendingData = Arc<Mutex<HashMap<u32, Sender<RpcResult>>>>;

/// `Client` represents a connection to some server that accepts
/// MessagePack RPC requests.
pub struct Client<'c, R, W> {
    input: PhantomData<R>, // the reader instance is owned by the dispatch thread
    output: W,
    pending: PendingData,
    dispatch_guard: JoinGuard<'c, ()>,
}

impl<'c, R, W> Client<'c, R, W> where R: Read + Send + 'c, W: Write + Send + 'c {
    /// Construct a new Client instance from a stream.
    ///
    /// In order for this to work properly, the stream's `.clone()` method must
    /// return a new handle to the shared underlying stream; in other words, it
    /// must be a shallow clone and not deep. `TCPStream` and `UnixStream` are known
    /// to do this, but other types should be aware of this restriction.
    pub fn new_for_stream<S>(stream: S) -> Client<'c, S, S> where S: Read + Write + Clone + Send + 'c {
        Client::new(stream.clone(), stream)
    }

    /// Construct a new Client instance from a reader/writer pair.
    pub fn new(reader: R, writer: W) -> Client<'c, R, W> {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        Client{
            input: PhantomData,
            output: writer,
            pending: pending.clone(),
            dispatch_guard: Client::<R, W>::dispatch_thread(reader, pending),
        }
    }

    /// Runs a scoped thread for dispatching messages received from reader to the appropriate parties.
    fn dispatch_thread(reader: R, pending: PendingData) -> JoinGuard<'c, ()> {
        thread::scoped(move || {
            let mut reader = super::Reader::new(reader);
            loop {
                match reader.read_value() {
                    Ok(value) => assert_eq!(value.int(), 1),
                    Err(ReadError::Unrecognized(0)) => break, // kill signal
                    Err(ReadError::NoData) => break, // eof
                    Err(e) => panic!("error: {}", e),
                }

                match reader.read_value() {
                    Ok(Value::Uint32(msgid)) => {
                        let responder: Sender<RpcResult> = pending.lock().unwrap().remove(&msgid).unwrap();

                        let err = reader.read_value().unwrap();
                        let res = reader.read_value().unwrap();

                        responder.send(if err == Value::Nil { Ok(res) } else { Err(err) }).unwrap();
                    },
                    Ok(value) => panic!("received value, but it wasn't a u32: {:?}", value),
                    Err(e) => panic!("failed to receive value: {:?}", e),
                }
            }
        })
    }

    /// Helper for calling methods via RPC.
    fn do_call(&mut self, method: String, params: Vec<Value>) -> Result<Receiver<RpcResult>, WriteError> {
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
    pub fn call(&mut self, method: String, params: Vec<Value>) -> Result<Future<RpcResult>, WriteError> {
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
}

#[unsafe_destructor]
impl<'c, R, W> Drop for Client<'c, R, W> where W: Write {
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
    use std::time::duration::Duration;

    use super::Client;
    use super::super::{Value, ReadError, write, write_value};

    // This test just calls a method "ping" and expects the result "pong".
    #[test] fn simple_test() {
        let (cw, cr) = channel();
        let (sw, sr) = channel();

        thread::spawn(move || {
            let mut reader = ::Reader::new(::test::ChanReader(cr));
            let mut writer = ::test::ChanWriter(sw);

            assert_eq!(reader.read_value().unwrap().int(), 0);
            let msgid = reader.read_value().unwrap().uint() as u32;
            let method = reader.read_value().unwrap().string();
            let _ = reader.read_value().unwrap().array(); // params

            match method.as_slice() {
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
        let mut value_future = client.call("ping".to_string(), vec![]).unwrap();
        let value = value_future.get().unwrap();
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
                    Ok(value) => assert_eq!(value.int(), 0),
                    Err(ReadError::Unrecognized(0)) => break,
                    Err(ReadError::NoData) => break,
                    x => panic!("received unexpected value '{:?}'", x),
                }

                let msgid = reader.read_value().unwrap().uint() as u32;
                let method = reader.read_value().unwrap().string();
                let params = reader.read_value().unwrap().array();
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

                    match method.as_slice() {
                        "quick_echo" => {
                            echo!();
                        },
                        "slow_echo" => {
                            use std::old_io::timer::sleep;
                            sleep(Duration::seconds(2));
                            echo!();
                        },
                        m => panic!("didn't expect you to call method '{}'", m),
                    }
                });
            }
        });

        let mut client = Client::new(::test::ChanReader(sr), ::test::ChanWriter(cw));

        let mut hello_future = client.call("slow_echo".to_string(), vec![Value::String("hello".to_string())]).unwrap();
        let mut world_future = client.call("quick_echo".to_string(), vec![Value::String("world".to_string())]).unwrap();

        assert_eq!(hello_future.get().unwrap(), Value::String("hello".to_string()));
        assert_eq!(world_future.get().unwrap(), Value::String("world".to_string()));
    }
}
