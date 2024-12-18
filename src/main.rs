use anyhow::{Ok, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use dotenv::dotenv;
use reqwest::Response;
use serde_json::Value;
use std::{env, fs::File, io::BufReader, sync::{atomic::{AtomicBool, Ordering}, Arc}};
use oauth1::{Token, authorize};

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
    let file = File::open(file).expect("file open failed.");
    let reader: BufReader<File> = BufReader::new(file);
    let value: serde_json::Value = serde_json::from_reader(reader).expect("file load failed.");
    value
}

async fn delete_tweet(id: u64, consumer_key: &str, consumer_secret: &str, access_token: &str, access_secret: &str) -> Result<Response, reqwest::Error> {
    let client = reqwest::Client::new();

    let url = format!(
        "https://api.x.com/1.1/statuses/destroy/{}.json", id
    );

    let consumer = Token::new(consumer_key, consumer_secret);
    let access = Token::new(access_token, access_secret);
    let authorize_header = authorize("POST", &url, &consumer, Some(&access), None);
    client
        .post(&url)
        .header("Authorization", authorize_header)
        .send()
        .await
}

async fn delete_task(id: u64, consumer_key: &str, consumer_secret: &str, access_token: &str, access_secret: &str) {
    loop {
        let response= delete_tweet(id, &consumer_key, &consumer_secret, &access_token, &access_secret)
            .await
            .expect(&format!("failed to delete post. id={}", id));
        if response.status().is_success() {
            println!("deleted. id={}", id);
            return;
        } else if response.status().as_u16() == 429 {
            if let Some(retry_after) = response.headers().get("Retry-After") {
                let retry_time_str = retry_after.to_str().expect("failed parse Retry-After value.");
                let retry_time = retry_time_str.parse::<u64>().expect("failed parse to u64.");

                println!("wait for rate limit. Retry-After={}", retry_time);
                tokio::time::sleep(tokio::time::Duration::from_secs(retry_time)).await;
            } else if let Some(reset_time) = response.headers().get("x-rate-limit-reset") {
                let timestamp_str = reset_time.to_str().expect("failed parse x-rate-limit-reset.");
                let timestamp = timestamp_str.parse::<i64>().expect("failed parse to i64");
                let naive = DateTime::from_timestamp(timestamp, 0).expect("invalid timestamp");

                let now = Utc::now();
                let sleep_duration = (naive - now).to_std().expect(&format!("failed calculate duration. naive={} now={}", naive, now));
                println!("wait till {}. x-rate-limit-reset={}", naive.to_string(), timestamp_str);
                tokio::time::sleep(sleep_duration).await;
            } else {
                // unknown. stop
                panic!("unknown 429 error");
            }
            continue;
        } else if response.status().as_u16() == 404 {
            // processed_dataから消す為に戻す
            println!("not found. id={}", id);
            return;
        } else {
            panic!("failed to delete post. id={} status={}", id, response.status());
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        println!("Ctrl+C received.");
        r.store(false, Ordering::SeqCst);
    }).expect("failed to set Ctrl+C handler.");

    dotenv().ok();
    let cli = Cli::parse();

    let consumer_key = env::var("CONSUMER_KEY").expect("CONSUMER_KEY not found in environment.");
    let consumer_secret = env::var("CONSUMER_SECRET").expect("CONSUMER_SECRET not found in environment.");
    let access_key = env::var("ACCESS_KEY").expect("ACCESS_KEY not found in environment.");
    let access_secret = env::var("ACCESS_SECRET").expect("ACCESS_SECRET not found in environment.");

    let tweets = get_tweets_data(&cli.tweets);
    let time = chrono::NaiveDate::parse_from_str(&cli.time, "%Y-%m-%d").expect("failed time parse. (format %Y-%m-%d)");
    let posts = {
        let data = tweets.as_array().expect("data isn't valid format.");
        let filtered_data: Vec<serde_json::Value> = data.iter().filter(|tweet| {
            let post_created_at = tweet["tweet"]["created_at"].as_str().expect("'created_at' not found.");
            let post_time = chrono::NaiveDate::parse_from_str(post_created_at, "%a %b %d %H:%M:%S %z %Y")
                .expect(&format!("parse failed. expect format (%a %b %d %H:%M:%S %z %Y). tweet_created_at={}", post_created_at));
            post_time < time
        }).cloned().collect();
        filtered_data
    };

    let mut processed_data = ProcessedValue::new(posts.clone(), cli.tweets.clone());

    for tweet in posts {
        if !running.load(Ordering::SeqCst) {
            println!("stop.");
            break;
        }
        let data = &tweet["tweet"];
        if *data != serde_json::Value::Null {
            let id = data["id"].as_str().expect("'id' not found");
            // check
            let id = id.parse::<u64>().expect(&format!("'id' isn't u64. id={}", id));

            delete_task(id, &consumer_key, &consumer_secret, &access_key, &access_secret).await;
            processed_data.process();
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    Ok(())
}