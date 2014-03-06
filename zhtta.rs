//
// zhtta.rs
//
// Starting code for PS3
// Running on Rust 0.9
//
// Note that this code has serious security risks!  You should not run it 
// on any system with access to sensitive files.
// 
// University of Virginia - cs4414 Spring 2014
// Weilin Xu and David Evans
// Version 0.5

// To see debug! outputs set the RUST_LOG environment variable, e.g.: export RUST_LOG="zhtta=debug"

#[feature(globs)];
extern mod extra;

use std::io::*;
use std::io::net::ip::{SocketAddr};
use std::{os, str, libc, from_str};
use std::path::Path;
use std::hashmap::HashMap;

use extra::getopts;
use extra::arc::MutexArc;
use extra::arc::RWArc;
use extra::sync::Semaphore;


pub mod gash;

static SERVER_NAME : &'static str = "Zhtta Version 0.5";

static IP : &'static str = "127.0.0.1";
static PORT : uint = 4414;
static WWW_DIR : &'static str = "./www";

static HTTP_OK : &'static str = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n";
static HTTP_BAD : &'static str = "HTTP/1.1 404 Not Found\r\n\r\n";

static COUNTER_STYLE : &'static str = "<doctype !html><html><head><title>Hello, Rust!</title>
             <style>body { background-color: #884414; color: #FFEEAA}
                    h1 { font-size:2cm; text-align: center; color: black; text-shadow: 0 0 4mm red }
                    h2 { font-size:2cm; text-align: center; color: black; text-shadow: 0 0 4mm green }
             </style></head>
             <body>";

struct HTTP_Request {
    // Use peer_name as the key to access TcpStream in hashmap. 

    // (Due to a bug in extra::arc in Rust 0.9, it is very inconvenient to use TcpStream without the "Freeze" bound.
    //  See issue: https://github.com/mozilla/rust/issues/12139)
    peer_name: ~str,
    path: ~Path,
    length: ~uint,
}

struct WebServer {
    ip: ~str,
    port: uint,
    www_dir_path: ~Path,
    
    request_queue_arc: MutexArc<~[HTTP_Request]>,
    stream_map_arc: MutexArc<HashMap<~str, Option<std::io::net::tcp::TcpStream>>>,
    
    notify_port: Port<()>,
    shared_notify_chan: SharedChan<()>,
}

impl WebServer {
    fn new(ip: &str, port: uint, www_dir: &str) -> WebServer {
        let (notify_port, shared_notify_chan) = SharedChan::new();
        let www_dir_path = ~Path::new(www_dir);
        os::change_dir(www_dir_path.clone());

        WebServer {
            ip: ip.to_owned(),
            port: port,
            www_dir_path: www_dir_path,
                        
            request_queue_arc: MutexArc::new(~[]),
            stream_map_arc: MutexArc::new(HashMap::new()),
            
            notify_port: notify_port,
            shared_notify_chan: shared_notify_chan,        
        }
    }
    
    fn run(&mut self) {
        let counter: RWArc<uint> = RWArc::new(0);
        let cache: RWArc<HashMap<~str, ~str>> = RWArc::new(HashMap::new());
        self.listen(counter);
        self.dequeue_static_file_request(cache);
    }
    
    fn listen(&mut self,counter:RWArc<uint>) {
        let addr = from_str::<SocketAddr>(format!("{:s}:{:u}", self.ip, self.port)).expect("Address error.");
        let www_dir_path_str = self.www_dir_path.as_str().expect("invalid www path?").to_owned();
        
        let request_queue_arc = self.request_queue_arc.clone();
        let shared_notify_chan = self.shared_notify_chan.clone();
        let stream_map_arc = self.stream_map_arc.clone();
                
        spawn(proc() {
            let mut acceptor = net::tcp::TcpListener::bind(addr).listen();
            println!("{:s} listening on {:s} (serving from: {:s}).", 
                     SERVER_NAME, addr.to_str(), www_dir_path_str);

            for stream in acceptor.incoming() {
                let (queue_port, queue_chan) = Chan::new();
                queue_chan.send(request_queue_arc.clone());
                counter.write(|count: &mut uint|{*count +=1});
                let (port, chan) = Chan::new();
                chan.send(counter.clone());
                let notify_chan = shared_notify_chan.clone();
                let stream_map_arc = stream_map_arc.clone();
                
                // Spawn a task to handle the connection.
                spawn(proc() {
                    let counter2=port.recv();

                    let request_queue_arc = queue_port.recv();
                  
                    let mut stream = stream;
                    
                    let peer_name = WebServer::get_peer_name(&mut stream);
                    
                    let mut buf = [0, ..500];
                    stream.read(buf);
                    let request_str = str::from_utf8(buf);
                    debug!("Request:\n{:s}", request_str);
                    
                    let req_group : ~[&str]= request_str.splitn(' ', 3).collect();
                    if req_group.len() > 2 {
                        let path_str = "." + req_group[1].to_owned();
                        
                        let mut path_obj = ~os::getcwd();
                        path_obj.push(path_str.clone());
                        
                        let ext_str = match path_obj.extension_str() {
                            Some(e) => e,
                            None => "",
                        };
                        
                        debug!("Requested path: [{:s}]", path_obj.as_str().expect("error"));
                        debug!("Requested path: [{:s}]", path_str);
                             
                        if path_str == ~"./" {
                            debug!("===== Counter Page request =====");
                            WebServer::respond_with_counter_page(stream,counter2);
                            debug!("=====Terminated connection from [{:s}].=====", peer_name);
                        } else if !path_obj.exists() || path_obj.is_dir() {
                            debug!("===== Error page request =====");
                            WebServer::respond_with_error_page(stream, path_obj);
                            debug!("=====Terminated connection from [{:s}].=====", peer_name);
                        } else if ext_str == "shtml" { // Dynamic web pages.
                            debug!("===== Dynamic Page request =====");
                            WebServer::respond_with_dynamic_page(stream, path_obj);
                            debug!("=====Terminated connection from [{:s}].=====", peer_name);
                        } else { 
                            debug!("===== Static Page request =====");
                            WebServer::enqueue_static_file_request(stream, path_obj, stream_map_arc, request_queue_arc, notify_chan);
                        }
                    }
                });
            }
        });
    }

    fn respond_with_error_page(stream: Option<std::io::net::tcp::TcpStream>, path: &Path) {
        let mut stream = stream;
        let msg: ~str = format!("Cannot open: {:s}", path.as_str().expect("invalid path").to_owned());

        stream.write(HTTP_BAD.as_bytes());
        stream.write(msg.as_bytes());
    }

    fn respond_with_counter_page(stream: Option<std::io::net::tcp::TcpStream>,counter:RWArc<uint>) {
        let mut counter2: uint=0;
        counter.read(|count| counter2=count.clone());
        let mut stream = stream;
        let response: ~str = 
            format!("{:s}{:s}<h1>Greetings, Krusty!</h1>
                     <h2>Visitor count: {:u}</h2></body></html>\r\n", 
                    HTTP_OK, COUNTER_STYLE, 
                    counter2);
        debug!("Responding to counter request");
        stream.write(response.as_bytes());
    }
    
    // TODO: Streaming file.
    // TODO: Application-layer file caching.
    fn respond_with_static_file(stream: Option<std::io::net::tcp::TcpStream>, path: &Path, cache: RWArc<HashMap<~str, ~str>>) {
        let mut stream = stream;
        //println("GOT HERE ");
        //println!("FILE PATH {:s}", path.display().to_str());
        //let mut cache: HashMap<~str, ~str> = HashMap::new();
        let mut mapout=HashMap::new();
        cache.read(|map|{mapout=map.clone()});

        if(mapout.contains_key(&(path.display().to_str())))
        {

            let file_cont: ~str = mapout.get(&(path.display().to_str())).to_owned();
            stream.write(file_cont.as_bytes());
        }
        else
        {
            let mut file_reader = File::open(path).expect("Invalid file!");

            stream.write(HTTP_OK.as_bytes());
            //stream.write(file_reader.read_to_end());
            let stat= fs::stat(path);
            let mut length=stat.size;
            let mut file_contents: ~str=~"";
            while(length>0)
            {
                if(length>1000)
                {
                    length -=1000;
                    let contents=file_reader.read_bytes(1000);
                    file_contents.push_str(str::from_utf8(contents));//str::from_utf8(contents);
                    stream.write(contents);

                }
                else
                {
                    length=0;
                    let contents=file_reader.read_to_end();
                    file_contents.push_str(str::from_utf8(contents));
                    //file_contents+=str::from_utf8(contents);
                    //file_contents.push(contents.clone());
                    stream.write(contents);

                }
            }
            //println!("Path FILE: {} FILE_CONETENDS :{}",path.display().to_str(),file_contents);
            //cache.access(|map| //{map.insert(path.display().to_str(),file_contents.clone())});
            mapout.insert(path.display().to_str(),file_contents.clone());
            cache.write(|map| {*map=mapout.clone()});
            //cache.read(|map|{println!("{}",map.to_str())});

        }
    }

    fn respond_with_dynamic_page(mut stream: Option<std::io::net::tcp::TcpStream>, path: &Path) {
        let mut fileContent = File::open(path).read_to_str();
                //println!("{:u}", fileContent.find_str("<!--").unwrap());

                let splitstr: ~[&str] = fileContent.split_str("<!--").collect();
                
                let mut counter: uint = 0;
                let mut command: ~str = ~"";

                while counter < splitstr.len(){
                    if splitstr[counter].contains("cmd"){

                        let findCmd: ~[&str] = splitstr[counter].split(' ').collect();
                            
                            let mut secCount: uint = 0;
                            while secCount < findCmd.len(){
                                if findCmd[secCount].contains("cmd"){
                                    command = findCmd[secCount].trim_right_chars(&'"').to_owned();
                                    command = command.trim_left_chars(&'"').to_owned();
                                    let lastCommand: ~[&str] = command.split('"').collect();
                                    command = lastCommand[1].to_owned();
                                }
                                secCount = secCount + 1;
                            }
                    }
                    counter = counter + 1;
                }
                
        let mut cmdOutput: ~str = ~"";
        match command{
            ~"" => { println("no command found")}
            _   => {
                    cmdOutput = gash::run_cmdline(command);
            }
        }
        
        let mut msg: ~str = format!("<!--\\#exec cmd='{:s}' -->", command);
        msg = msg.replace("'", "\"");

        if cmdOutput != ~"" {fileContent = fileContent.replace(msg,cmdOutput);}

        stream.write(fileContent.as_bytes());

    }
    
    // TODO: Smarter Scheduling.
    fn enqueue_static_file_request(stream: Option<std::io::net::tcp::TcpStream>, path_obj: &Path, stream_map_arc: MutexArc<HashMap<~str, Option<std::io::net::tcp::TcpStream>>>, req_queue_arc: MutexArc<~[HTTP_Request]>, notify_chan: SharedChan<()>) {
        // Save stream in hashmap for later response.
        //let stat= fs::stat(path_obj);
        let length= File::open(path_obj).read_to_end().len();

        let mut stream = stream;
        let peer_name = WebServer::get_peer_name(&mut stream);
        //println!("peer_NAME: {}",peer_name);
        let (stream_port, stream_chan) = Chan::new();
        stream_chan.send(stream);
        unsafe {
            // Use an unsafe method, because TcpStream in Rust 0.9 doesn't have "Freeze" bound.
            stream_map_arc.unsafe_access(|local_stream_map| {
                let stream = stream_port.recv();
                local_stream_map.swap(peer_name.clone(), stream);
            });
        }
        
        // Enqueue the HTTP request.
        let req = HTTP_Request { peer_name: peer_name.clone(), path: ~path_obj.clone(), length: ~length.clone() };
        let (req_port, req_chan) = Chan::new();
        req_chan.send(req);

        debug!("Waiting for queue mutex lock.");
        req_queue_arc.access(|local_req_queue| {
            debug!("Got queue mutex lock.");
            let req: HTTP_Request = req_port.recv();
        if(peer_name.contains("128.143.")||peer_name.contains("132.54."))
        {
            if(local_req_queue.len()==0)
            {
                 local_req_queue.push(req);
            }
            else
            {
                for i in range(0, local_req_queue.len())
                {
                    if(!local_req_queue[i].peer_name.contains("128.143.")&&!local_req_queue[i].peer_name.contains("132.54."))
                    {
                        local_req_queue.insert(i,req);
                        break;
                    }
                    else
                    {
                        if(~length>local_req_queue[i].length)
                        {
                            local_req_queue.insert(i,req);
                            break;
                        }
                        if i==local_req_queue.len()-1
                        {
                            local_req_queue.push(req);
                            break;
                        }
                    }
            
                }
            }
        }
        else
        {
            //println!("HELLO {} ",local_req_queue.len());
            if(local_req_queue.len()==0)
            {
                 local_req_queue.push(req);
            }
            else
            {
                for i in range(0, local_req_queue.len())
                {
                    if(!local_req_queue[i].peer_name.contains("128.143.")&&!local_req_queue[i].peer_name.contains("132.54.")&&~length<local_req_queue[i].length)
                    {
                        local_req_queue.insert(i,req);
                        break;
                    }
                    else
                    {
                        if i==local_req_queue.len()-1
                        {
                            local_req_queue.push(req);
                            break;
                        }
                    }
            
                }
            }

        }
            //println!("HERE: {}", local_req_queue.len());
            //local_req_queue.push(req);
            debug!("A new request enqueued, now the length of queue is {:u}.", local_req_queue.len());
        });
        
        notify_chan.send(()); // Send incoming notification to responder task.
    
    
    }
    
    // TODO: Smarter Scheduling. // Not sure what else to do for smarter scheduling
    fn dequeue_static_file_request(&mut self,cache: RWArc<HashMap<~str, ~str>>) {
        let req_queue_get = self.request_queue_arc.clone();
        let stream_map_get = self.stream_map_arc.clone();
        
        // Port<> cannot be sent to another task. So we have to make this task as the main task that can access self.notify_port.
        let(proc3, chan3)=Chan::new();
        chan3.send(cache);
        let (request_port, request_chan) = Chan::new();
        loop {
            self.notify_port.recv();    // waiting for new request enqueued.
            let mut done=false;
            let resources= Semaphore::new(4);
            req_queue_get.access( |req_queue| {
                match req_queue.shift_opt() { // FIFO queue.
                    None => { done=true; }
                    Some(req) => {
                        request_chan.send(req);
                        resources.acquire();
                        debug!("A new request dequeued, now the length of queue is {:u}.", req_queue.len());
                    }
                }
            });
            let(proc1, chan1)=Chan::new();

            let request = request_port.recv();

            // Get stream from hashmap.
            // Use unsafe method, because TcpStream in Rust 0.9 doesn't have "Freeze" bound.
            let (stream_port, stream_chan) = Chan::new();
            unsafe {
                stream_map_get.unsafe_access(|local_stream_map| {
                    let stream = local_stream_map.pop(&request.peer_name).expect("no option tcpstream");
                    stream_chan.send(stream);
                });
            }

            let(proc2, chan2)=Chan::new();
            chan2.send(proc3.recv());
            // TODO: Spawning more tasks to respond the dequeued requests concurrently. You may need a semophore to control the concurrency.
            spawn(proc() {
                let stream = stream_port.recv();
                WebServer::respond_with_static_file(stream, request.path, proc2.recv());
                // Close stream automatically.
                debug!("=====Terminated connection from [{:s}].=====", request.peer_name);
                chan1.send("yes");

            });
            if(proc1.recv()=="yes")
            {
                resources.release();
            }
            }
        }

    
    fn get_peer_name(stream: &mut Option<std::io::net::tcp::TcpStream>) -> ~str {
        match *stream {
            Some(ref mut s) => {
                         match s.peer_name() {
                            Some(pn) => {pn.to_str()},
                            None => (~"")
                         }
                       },
            None => (~"")
        }
    }
}

fn get_args() -> (~str, uint, ~str) {
    fn print_usage(program: &str) {
        println!("Usage: {:s} [options]", program);
        println!("--ip     \tIP address, \"{:s}\" by default.", IP);
        println!("--port   \tport number, \"{:u}\" by default.", PORT);
        println!("--www    \tworking directory, \"{:s}\" by default", WWW_DIR);
        println("-h --help \tUsage");
    }
    
    /* Begin processing program arguments and initiate the parameters. */
    let args = os::args();
    let program = args[0].clone();
    
    let opts = ~[
        getopts::optopt("ip"),
        getopts::optopt("port"),
        getopts::optopt("www"),
        getopts::optflag("h"),
        getopts::optflag("help")
    ];

    let matches = match getopts::getopts(args.tail(), opts) {
        Ok(m) => { m }
        Err(f) => { fail!(f.to_err_msg()) }
    };

    if matches.opt_present("h") || matches.opt_present("help") {
        print_usage(program);
        unsafe { libc::exit(1); }
    }
    
    let ip_str = if matches.opt_present("ip") {
                    matches.opt_str("ip").expect("invalid ip address?").to_owned()
                 } else {
                    IP.to_owned()
                 };
    
    let port:uint = if matches.opt_present("port") {
                        from_str::from_str(matches.opt_str("port").expect("invalid port number?")).expect("not uint?")
                    } else {
                        PORT
                    };
    
    let www_dir_str = if matches.opt_present("www") {
                        matches.opt_str("www").expect("invalid www argument?") 
                      } else { WWW_DIR.to_owned() };
    
    (ip_str, port, www_dir_str)
}

fn main() {
    let (ip_str, port, www_dir_str) = get_args();
    let mut zhtta = WebServer::new(ip_str, port, www_dir_str);
    zhtta.run();
}
