#[macro_use]
extern crate log;
extern crate env_logger;

// md parser + formatter
extern crate comrak;
// simple http server
extern crate simple_server;

// cmdline parsing
extern crate structopt;

use comrak::ComrakOptions;
use simple_server::Server;
use std::fs::File;
use std::io::Read;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process;
use structopt::StructOpt;

use inotify::{Inotify, WatchMask, EventMask};

#[derive(Debug, StructOpt)]
/// grup - a offline github markdown previewer
struct Cfg {
    #[structopt(name = "markdown_file", parse(from_os_str))]
    /// The markdown file to be served
    md_file: PathBuf,
    #[structopt(
        long = "port",
        default_value = "8000",
        help = "the port to use for the server"
    )]
    port: u16,
    #[structopt(
        long = "host",
        default_value = "127.0.0.1",
        help = "the ip to use for the server"
    )]
    host: IpAddr,
}

const DEFAULT_CSS: &[u8] = include_bytes!("../resource/github-markdown.css");

fn main() {
    env_logger::Builder::from_default_env().init();
    let cfg = Cfg::from_args();
    let file = cfg.md_file;

    // these were parsed and checked by structopt but now we need to turn them back to strings
    let (host, port) = (format!("{}", cfg.host), format!("{}", cfg.port));

    if !file.exists() {
        eprintln!("Error: {:#?} does not exist!", file);
        process::exit(-1);
    }

    if !file.is_file() {
        eprintln!("Error: {:#?} is not a file!", file);
        process::exit(-1);
    }

    let mut inotify = Inotify::init().expect("inotify init failed");
    let parent = if let Some(parent) = file.parent() {
        if parent.to_str().unwrap() != "" {
            parent
        } else {
            std::path::Path::new(".")
        }
    } else {
        std::path::Path::new(".")
    };

    info!("parent {:?}", parent);

    inotify.add_watch(&parent, WatchMask::MODIFY | WatchMask::CREATE).expect("failed to watch");

    let modified = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    std::thread::spawn( {
        let file = file.clone();
        let modified = modified.clone();
        move ||{
            loop {
                let mut buf = [0u8; 1024];
                let events = inotify.read_events_blocking(&mut buf).expect("failed to read events");
                for event in events {
                    if event.mask.contains(EventMask::CREATE) {
                        info!("file created {:?}", event.name.unwrap());
                    } else if event.mask.contains(EventMask::MODIFY) {
                        info!("file modified {:?}", event.name.unwrap());
                    }
                    if &event.name.unwrap() == &file.to_str().unwrap() {
                        modified.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }
    });

    let mut server = Server::new(move |request, mut response| {
        info!("Request received. {} {}", request.method(), request.uri());

        let interval = 60;

        if request.uri().path() == "/update" {
            for _i in 0..interval {
                if modified.compare_and_swap(true, false, std::sync::atomic::Ordering::Relaxed) == true {

                    return Ok(response.body("yes".as_bytes().to_vec())?);
                }
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
            return Ok(response.body("no".as_bytes().to_vec())?);
        }

        // if they want the stylesheet serve it
        // else give them the formatted MD file
        if request.uri().path() == "/style.css" {
            return Ok(response.body(DEFAULT_CSS.to_vec())?);
        }

        let parsed_and_formatted = File::open(&file)
            .and_then(|mut f| {
                let mut s = String::new();
                f.read_to_string(&mut s).map(|_| s)
            })
            .and_then(|md| {
                let mut options = ComrakOptions::default();
                options.hardbreaks = true;
                Ok(comrak::markdown_to_html(&md, &options))
            })
            .unwrap_or_else(|e| format!("Grup encountered an error: <br> {:#?}", e));

        let title = String::from(file.to_str().unwrap_or(&format!("{:?}", file)));

        // push it all into a container
        let doc = format!(
            r#"<!DOCTYPE html>
             <html>
                <head>
                    <meta http-equiv="Content-Type" content="text/html; charset=utf-8"/>
                    <style>
                        body {{
                        box-sizing: border-box;
                        min-width: 200px;
                        max-width: 980px;
                        margin: 0 auto;
                        padding: 45px;
                        }}
                    </style>
                    <link rel="stylesheet" href="style.css">
                    <title>{}</title>
                </head>
                <body>
                <article class="markdown-body">
                {}
                <article class="markdown-body">
                <script type="text/javascript">
                function reload_check () {{
                    var xhr = new XMLHttpRequest();
                    xhr.overrideMimeType("text/plain");
                    xhr.onreadystatechange = function () {{
                        if (this.status == 200) {{
                            if (this.responseText == "yes") {{
                                location.reload();
                            }}
                        }}
                    }}
                    xhr.open("GET", "/update", true);
                    xhr.send();
                }}
                reload_check();
                window.setInterval(reload_check, {});
                </script>
                </body>
            </html>"#,
            title, parsed_and_formatted, interval*1000
        );

        Ok(response.body(doc.into_bytes())?)
    });

    server.set_static_directory(".");

    println!("Server running at http://{}:{}", host, port);
    println!("Press Ctrl-C to exit");
    server.listen(&host, &port);
}
