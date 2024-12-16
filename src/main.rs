use anyhow::{Ok, Result};
use clap::Parser;
use dotenv::dotenv;
use reqwest::Response;
use serde_json::Value;
use std::{env, fs::File, io::BufReader, process::exit};

struct ProcessedValue {
    data: Vec<Value>,
    name: String,
}

impl ProcessedValue {
    fn new(data: Vec<Value>, name: String) -> Self {
        Self {
            data,
            name
        }
    }

    fn process(&mut self) {
        if !self.data.is_empty() {
            self.data.remove(0);
        }
    }
}

impl Drop for ProcessedValue {
    fn drop(&mut self) {
        match File::create(self.name.clone()) {
            std::result::Result::Ok(file) => {
                serde_json::to_writer(file, &self.data).unwrap_or_else(|err| {
                    eprintln!("failed to write {}. err={}", self.name, err);
                });
            },
            Err(err) => eprintln!("failed to create {}. err={}", self.name, err),
        };
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    tweets: String,
    time: String,
}

fn get_tweets_data(file: &str) -> serde_json::Value {
    let file = File::open(file).unwrap_or_else(|err| {
        eprintln!("file load failed. {}", err);
        exit(line!() as i32);
    });
    let reader: BufReader<File> = BufReader::new(file);
    let value: serde_json::Value = serde_json::from_reader(reader).unwrap_or_else(|err| {
        eprintln!("failed file load. {}", err);
        exit(line!() as i32);
    });
    value
}

async fn delete_tweet(id: u64, key: &str, secret: &str) -> Result<Response, reqwest::Error> {
    let client = reqwest::Client::new();

    let delete_url = format!(
        "https://api.twitter.com/2/tweets/{}", id
    );

    client
        .delete(&delete_url)
        .header("Authorization", format!("OAuth oauth_consumer_key={}, oauth_consumer_secret={}", key, secret))
        .send()
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let cli = Cli::parse();

    let consumer_key = env::var("CONSUMER_KEY")
        .expect("CONSUMER_KEY must be set");
    let consumer_secret = env::var("CONSUMER_SECRET")
        .expect("CONSUMER_SECRET must be set");

    let tweets = get_tweets_data(&cli.tweets);
    let time = chrono::NaiveDate::parse_from_str(&cli.time, "%Y-%m-%d").unwrap_or_else(|err| {
        eprintln!("failed time parse (format Y-m-d). {}", err);
        exit(line!() as i32);
    });

    let posts = match tweets.as_array() {
        Some(data) => {
            let filtered_data: Vec<serde_json::Value> = data.iter().filter(|tweet| {
                let post_created_at = tweet["tweet"]["created_at"].as_str().unwrap_or_else(|| {
                    eprintln!("'created_at' not found.");
                    exit(line!() as i32);
                });
                let post_time = chrono::NaiveDate::parse_from_str(post_created_at, "%a %b %d %H:%M:%S %z %Y")
                    .unwrap_or_else(|err| {
                    eprintln!("parse failed. tweet_created_at={} err={}", post_created_at, err);
                    exit(line!() as i32);
                });
                post_time > time
            }).cloned().collect();
            filtered_data
        },
        None => {
            eprintln!("data isnt valid format.");
            exit(line!() as i32);
        },
    };

    let mut processed_data = ProcessedValue::new(posts.clone(), cli.tweets.clone());

    for tweet in posts {
        let data = &tweet["tweet"];
        if *data != serde_json::Value::Null {
            let id = data["id"].as_str().unwrap_or_else(||{
                eprintln!("'id' not found");
                exit(line!() as i32);
            });
            // check
            let id = id.parse::<u64>().unwrap_or_else(|err| {
                eprintln!("'id' isn't u64. id={} err={}", id, err);
                exit(line!() as i32);
            });

            match delete_tweet(id, &consumer_key, &consumer_secret).await {
                core::result::Result::Ok(response) => {
                    if response.status().is_success() {
                        let json_response: serde_json::Value = match response.json().await {
                            std::result::Result::Ok(data) => data,
                            Err(err) => {
                                eprintln!("failed to decode json. err={}", err);
                                break;
                            },
                        };
                        if json_response["data"]["deleted"].as_bool().unwrap_or(false) {
                            println!("deleted. id={}", id);
                            processed_data.process();
                        } else {
                            eprintln!("faile to delete post. id={}", id);
                            break;
                        }
                    } else {
                        eprintln!("failed to delete post. id={} status={}", id, response.status());
                        break;
                    }
                },
                Err(err) => {
                    eprintln!("Error deleting tweet ID: {}. Error: {}", id, err);
                    break;
                }
            }
        }
    }

    Ok(())
}