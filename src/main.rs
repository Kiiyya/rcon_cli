#[macro_use]
extern crate crossterm;

use std::{io::{BufRead, Write, stdin, stdout}, process::exit};
use ascii::IntoAsciiString;
use clap::{Arg, SubCommand};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use dotenv::dotenv;
use tokio_stream::{Stream, StreamExt};
use chrono::Utc;

use battlefield_rcon::{
    bf4::{
        error::{Bf4Error, Bf4Result},
        Bf4Client, Event,
    },
    rcon::{RconConnectionInfo, RconError, RconQueryable, RconResult},
};

#[tokio::main]
async fn main() -> RconResult<()> {
    dotenv().ok(); // load (additional) environment variables from `.env` file in working directory.

    let matches = clap::App::new("rcon_cli")
        .version("0.1.1")
        .about("Extremely simple and BF4-specifics-unaware (yet) library to send and receive strings. Hint: I also read in environment variables (one per line) from a .env file in the current working directory or up!")
        .author("Kiiya (snoewflaek@gmail.com, Discord: Kiiya#0456)")
        .arg(Arg::with_name("raw")
            .short("r")
            .long("--raw")
            .takes_value(false)
            .help("Prevents color output and ->, <-. Use this for automated scripts")
        )
        .arg(Arg::with_name("rcon_ip")
            .env("BFOX_RCON_IP")
            .long("--ip")
            .takes_value(true)
            .required(true)
            .help("Sets the RCON IP")
        )
        .arg(Arg::with_name("rcon_port")
            .env("BFOX_RCON_PORT")
            .long("--port")
            .required(true)
            .takes_value(true)
            .help("Sets the RCON port")
        )
        .arg(Arg::with_name("rcon_password")
            .env("BFOX_RCON_PASSWORD")
            .long("--password")
            .required(true)
            .takes_value(true)
            .help("Sets the RCON password. If possible, please use an environment variable or .env file instead!")
        )
        .subcommand(SubCommand::with_name("query")
            .about("Send single query and print result, instead of going into interactive mode")
            .arg(Arg::with_name("query-words").min_values(1))
        )
        .subcommand(SubCommand::with_name("events")
            .about("Simply dump all events")
            .arg(Arg::with_name("show-punkbuster").takes_value(true).help("Whether to show PunkBuster messages in dump").long("--punkbuster").default_value("no").possible_values(&["yes", "no"]))
        )
        .get_matches();

    // raw => no fancy colorful output.
    let raw = matches.is_present("raw");

    // fetch connection info from env vars and/or command line arguments.
    let password = matches.value_of("rcon_password").unwrap();
    let coninfo = RconConnectionInfo {
        ip: matches.value_of("rcon_ip").unwrap().to_owned(),
        port: matches
            .value_of("rcon_port")
            .unwrap()
            .parse::<u16>()
            .expect("Could not parse port number"),
        password: password.into_ascii_string().unwrap_or_else(|_| panic!("Could not parse password: \"{}\" is not an ASCII string", password)),
    };

    // connect to rcon
    let bf4 = match Bf4Client::connect(&coninfo).await {
        Ok(bf4) => bf4,
        Err(err) => {
            println!(
                "Failed to connect to Rcon at {}:{} with password ***: {:?}",
                coninfo.ip, coninfo.port, err
            );
            exit(-1);
        }
    };

    let (subcommand, subcommand_matches) = matches.subcommand();
    match subcommand {
        "query" => single_query(&subcommand_matches.unwrap(), &bf4, raw).await?,
        "events" => {
            let matcher = subcommand_matches.unwrap();
            let show_pb = matcher
                .value_of("show-punkbuster")
                .map(|val| match val {
                    "yes" => true,
                    "no" => false,
                    _ => panic!("clap should have caught this case..."),
                })
                .unwrap();
            events_dump(raw, bf4.event_stream().await?, show_pb).await?;
        }
        _ => match interactive(raw, bf4).await {
            Err(RconError::ConnectionClosed) => Ok(()), // if the error was connection closed, then, so be it!
            other => other
        }?
    }

    Ok(())
}

async fn events_dump(
    raw: bool,
    stream: impl Stream<Item = Bf4Result<Event>> + Unpin,
    show_pb: bool,
) -> RconResult<()> {
    let mut stream = stream.filter(|ev| match ev {
        Ok(Event::PunkBusterMessage(_)) => show_pb,
        _ => true,
    });

    while let Some(event) = stream.next().await {
        match event {
            Ok(ev) => println!("{} {:?}", Utc::now(), ev),
            Err(Bf4Error::UnknownEvent(vec)) => {
                // TODO make fancy colors with UNKNWON EVENT RAWR once I have enough events implemented in battlefield_rcon.
                println!("{} {:?}", Utc::now(), vec);
            }
            Err(err) => {
                if raw {
                    println!("!!! Error {:?}", err);
                } else {
                    execute!(
                        stdout(),
                        SetForegroundColor(Color::Black),
                        SetBackgroundColor(Color::Red),
                        Print("!!! Error".to_string()),
                        SetForegroundColor(Color::Red),
                        SetBackgroundColor(Color::Reset),
                        Print(format!(" {:?}", err)),
                        ResetColor
                    )
                    .unwrap();
                }
            }
        }
    }
    todo!()
}

async fn interactive(raw: bool, bf4: std::sync::Arc<Bf4Client>) -> RconResult<()> {
    if !raw {
        print!("-> ");
        stdout().flush()?;
    }
    let stdin = stdin();
    let x = stdin.lock().lines();
    for line in x {
        let line = line?;
        let words = line.split(' ');
        handle_input_line(words, &bf4, raw).await?;

        if !raw {
            print!("-> ");
            stdout().flush()?;
        }
    }

    Ok(())
}

#[allow(clippy::needless_lifetimes)] // fuck you clippy, rustc doesn't think lifetimes are useless here!
async fn single_query<'a>(
    singlequery: &clap::ArgMatches<'a>,
    bf4: &std::sync::Arc<Bf4Client>,
    raw: bool,
) -> RconResult<()> {
    let words = singlequery
        .values_of("query-words")
        .unwrap()
        .collect::<Vec<_>>();
    handle_input_line(words, bf4, raw).await?;
    Ok(())
}

async fn handle_input_line(
    words: impl IntoIterator<Item = &str>,
    bf4: &Bf4Client,
    raw: bool,
) -> RconResult<()> {
    let mut words_ascii = Vec::new();
    for word in words {
        words_ascii.push(word.into_ascii_string()?);
    }
    let result = bf4
        .get_underlying_rcon_client()
        .query(
            &words_ascii,
            |ok| Ok(ok.to_owned()),
            |err| Some(RconError::other(err)),
        )
        .await;
    match result {
        Ok(ok) => {
            let mut str = String::new();
            for word in ok {
                str.push(' ');
                str.push_str(word.as_str());
            }
            if raw {
                println!("OK {}", str);
            } else {
                execute!(
                    stdout(),
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::Green),
                    Print("<- OK".to_string()),
                    SetForegroundColor(Color::Green),
                    SetBackgroundColor(Color::Reset),
                    Print(str),
                    ResetColor,
                    Print("\n".to_string())
                )
                .unwrap();
            }
        }
        Err(err) => {
            if !raw {
                execute!(
                    stdout(),
                    SetForegroundColor(Color::Black),
                    SetBackgroundColor(Color::Red),
                )
                .unwrap();
            }

            match err {
                RconError::Other(str) => {
                    // println!("{}", str.on_dark_red());
                    if raw {
                        println!("Error: {}", str);
                    } else {
                        execute!(
                            stdout(),
                            Print("<- Error".to_string()),
                            SetForegroundColor(Color::Red),
                            SetBackgroundColor(Color::Reset),
                            Print(" ".to_string()),
                            Print(str)
                        )
                        .unwrap();
                    }
                }
                RconError::ConnectionClosed => {
                    print_error_type("Connection Closed", raw).unwrap();
                    if !raw {
                        execute!(stdout(), ResetColor, Print("\n".to_string())).unwrap();
                    }
                    return Err(RconError::ConnectionClosed);
                }
                RconError::InvalidArguments { our_query: _ } => {
                    print_error_type("Invalid Arguments", raw).unwrap();
                }
                RconError::UnknownCommand { our_query: _ } => {
                    print_error_type("Unknown Command", raw).unwrap();
                }
                _ => panic!("Unexpected error: {:?}", err),
            };
            if !raw {
                execute!(stdout(), ResetColor, Print("\n".to_string())).unwrap();
            }
        }
    }

    Ok(())
}

fn print_error_type(typ: &str, raw: bool) -> Result<(), crossterm::ErrorKind> {
    if raw {
        println!("{}", typ);
        Ok(())
    } else {
        execute!(
            stdout(),
            SetBackgroundColor(Color::DarkRed),
            Print("<- ".to_string()),
            Print(typ),
        )
    }
}
