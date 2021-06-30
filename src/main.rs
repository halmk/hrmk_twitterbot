use serde::{Serialize, Deserialize};
use serde_json::Value;
use std::{collections::HashMap, fs::File, io::{BufReader, Write}};
use chrono::{Date, DateTime, Duration, TimeZone, Utc};
use cron::Schedule;
use std::str::FromStr;

#[tokio::main]
async fn main() {
    // config file path setting
    let home_path = dirs::home_dir().unwrap();
    let config = std::path::Path::new(&home_path);
    let config = config.join(".twitter").join("config");
    println!("{:?}", config);

    let schedule = Schedule::from_str("0 0 * * * * *").unwrap();

    while let Some(datetime) = schedule.upcoming(Utc).next() {
        let current_time = Utc::now();
        println!("{} -> {}", current_time, datetime);
        let duration = datetime - current_time;
        tokio::time::sleep(tokio::time::Duration::from_millis(duration.num_milliseconds() as u64)).await;
        job(config.to_str().unwrap()).await
            .unwrap_or_else(|err| println!("{:?}", err));
    }
}

async fn job(config: &str) -> Result<(), Box<dyn std::error::Error>> {
    // read tasks.json and get latest task timestamp
    let home_path = dirs::home_dir().unwrap();
    let tasks_filename = std::path::Path::new(&home_path);
    let tasks_filename = tasks_filename.join(".twitter").join("tasks.json");
    let file = File::open(tasks_filename).unwrap();
    let reader = BufReader::new(file);
    let mut tasks:Tasks = serde_json::from_reader(reader).unwrap();
    //println!("{:#?}", tasks);
    let timestamp = tasks.0.iter().map(|task| {
        task.timestamp
    }).max().unwrap_or(0i64);
    //println!("{:?}", timestamp);

    // read config file and use Twitter API
    match Client::from_config(config) {
        Ok(client) => {
            //println!("{:?}", client);
            //let response = client.tweet("ello").await?;

            // use mentions timeline api to get new task
            let response = client.mentions_timeline().await.unwrap();
            let text:Obj = response.json().await.unwrap();
            //println!("{:#?}", text);
            for data in text.0 {
                let message = data.get("text").unwrap();
                let message:Vec<&str> = message.as_str().unwrap().split_whitespace().collect();
                let user = data.get("user").unwrap().get("screen_name").unwrap().as_str().unwrap();
                let id = data.get("id_str").unwrap().as_str().unwrap();
                let created_at = match data.get("created_at").unwrap().as_str() {
                    Some(t) => {
                        Utc.datetime_from_str(t, "%a %b %d %T %z %Y")
                            .unwrap_or(t.parse().unwrap_or(Utc.timestamp(0,0)))
                    }
                    None => Utc.timestamp(0,0),
                };
                let t = created_at.timestamp();
                let title = message[1];
                let comment = message.iter().skip(2).map(|&s| s).collect::<Vec<_>>().join(" ");

                if t > timestamp {
                    println!("\n{:#?}", user);
                    println!("{:?}", id);
                    println!("{:?}, {:?}", created_at, t);
                    println!("{:#?}\n", message);

                    tasks.0.push(Task::new(id.to_string(), t, user.to_string(), title.to_string(), comment));
                }
            }

            // use tweet api to remind task for user
            let current_date = Utc::now();
            for task in tasks.0.iter_mut() {
                let mut next_date = Utc.datetime_from_str(&task.next_date, "%Y-%m-%d %H:%M:%S %Z").unwrap();
                //println!("{:?}", next_date);
                let dt = current_date - next_date.with_timezone(&Utc);
                //println!("{:?}", dt);
                if dt.num_seconds() > 0 {
                    let status = format!("@{}\n{}\n{}", task.user, task.title, task.comment);
                    println!("\nTweet: {}\n", status);
                    let _response = client.tweet(&status).await?;
                    next_date = next_date + Duration::days(7);
                    println!("next_date: {} -> {}", task.next_date, next_date.to_string());
                    task.next_date = next_date.to_string();
                }
            }
        }
        Err(err) => {
            println!("failed to read config file: {}", err);
        }

    }

    let json = serde_json::to_string(&tasks).unwrap();
    //println!("{:?}", json);
    let home_path = dirs::home_dir().unwrap();
    let tasks_filename = std::path::Path::new(&home_path);
    let tasks_filename = tasks_filename.join(".twitter").join("tasks.json");
    let mut new_json = File::create(tasks_filename).unwrap();
    new_json.write_all(json.as_bytes()).unwrap();

    Ok(())
}

#[derive(Debug, Deserialize)]
struct Obj(Vec<HashMap<String,Value>>);

#[derive(Debug, Serialize, Deserialize)]
struct Tasks(Vec<Task>);

#[derive(Debug, Serialize, Deserialize)]
struct Task {
    id: String,
    timestamp: i64,
    user: String,
    title: String,
    comment: String,
    date: String,
    next_date: String,
    count: i64,
}

impl Task {
    fn new(id:String, timestamp:i64, user:String, title:String, comment:String) -> Self {
        Self {
            id,
            timestamp,
            user,
            title,
            comment,
            date: Utc.timestamp(timestamp, 0).to_string(),
            next_date: (Utc.timestamp(timestamp, 0) + Duration::hours(24)).to_string(),
            count: 1,
        }
    }
}

#[derive(Debug)]
struct Client {
    api_key: String,
    api_secret_key: String,
    access_token: String,
    access_token_secret: String
}

impl Client {
    fn from_config(filename: &str) -> Result<Client, Box<dyn std::error::Error>> {
        let config = std::fs::File::open(filename)?;
        let mut reader = std::io::BufReader::new(config);
        fn read_line<T: std::io::BufRead>(reader: &mut T) -> Result<String, Box<dyn std::error::Error>> {
            let mut s = String::new();
            reader.read_line(&mut s)?;
            s.pop();
            Ok(s)
        }
        Ok(Client {
            api_key: read_line(&mut reader)?,
            api_secret_key: read_line(&mut reader)?,
            access_token: read_line(&mut reader)?,
            access_token_secret: read_line(&mut reader)?,
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        parameters: &std::collections::BTreeMap<&str, &str>,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let header_map = {
            use reqwest::header::*;
            let mut map = HeaderMap::new();
            map.insert(
                AUTHORIZATION,
                self.authorization(&method, url, parameters)
                        .parse()
                        .unwrap(),
            );
            map.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            map
        };
        let url_with_parameters = format!(
            "{}?{}",
            url,
            equal_collect(parameters.iter().map(|(key, value)| { (*key, *value) })).join("&")
        );

        //println!("{:#?}", url_with_parameters);
        //println!("{:#?}", header_map);
        let client = reqwest::Client::new();
        client
            .request(method, &url_with_parameters)
            .headers(header_map)
            .send()
            .await
    }

    async fn tweet(&self, status: &str) -> Result<reqwest::Response, reqwest::Error> {
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert("status", status);
        self.request(
            reqwest::Method::POST,
            "https://api.twitter.com/1.1/statuses/update.json",
            &parameters,
        )
        .await
    }

    async fn mentions_timeline(&self) -> Result<reqwest::Response, reqwest::Error> {
        let parameters = std::collections::BTreeMap::new();
        //parameters.insert("status", status);
        self.request(
            reqwest::Method::GET,
            "https://api.twitter.com/1.1/statuses/mentions_timeline.json",
            &parameters,
        )
        .await
    }

    fn authorization(
        &self,
        method: &reqwest::Method,
        url: &str,
        parameters: &std::collections::BTreeMap<&str, &str>,
    ) -> String {
        let timestamp = format!("{}", chrono::Utc::now().timestamp());
        let nonce: String = {
            use rand::prelude::*;
            let mut rng = thread_rng();
            std::iter::repeat(())
                .map(|()| rng.sample(rand::distributions::Alphanumeric))
                .take(32)
                .collect()
        };

        let mut other_parameters: Vec<(&str, &str)> = vec![
            ("oauth_consumer_key", &self.api_key),
            ("oauth_token", &self.access_token),
            ("oauth_signature_method", "HMAC-SHA1"),
            ("oauth_version", "1.0"),
            ("oauth_timestamp", &timestamp),
            ("oauth_nonce", &nonce),
        ];

        let signature = self.signature(method, url, parameters.clone(), &other_parameters);

        other_parameters.push(("oauth_signature", &signature));

        format!(
            "OAuth {}",
            equal_collect(other_parameters.into_iter()).join(", ")
        )
    }

    fn signature<'a>(
        &self,
        method: &reqwest::Method,
        url: &str,
        mut parameters: std::collections::BTreeMap<&'a str, &'a str>,
        other_parameters: &Vec<(&'a str, &'a str)>,
    ) -> String {
        for (key, value) in other_parameters {
            parameters.insert(key, value);
        }
        let parameters_string = equal_collect(parameters.into_iter()).join("&");

        let signature_base_string = format!(
            "{}&{}&{}",
            method,
            percent_encode(url),
            percent_encode(&parameters_string)
        );
        let signing_key = format!("{}&{}", self.api_secret_key, self.access_token_secret);
        base64::encode(hmacsha1::hmac_sha1(
            signing_key.as_bytes(),
            signature_base_string.as_bytes(),
        ))
    }
}

fn percent_encode(s: &str) -> percent_encoding::PercentEncode {
    use percent_encoding::*;
    const FRAGMENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'*')
        .remove(b'-')
        .remove(b'.')
        .remove(b'_');
    utf8_percent_encode(s, FRAGMENT)
}

fn equal_collect<'a, T: Iterator<Item = (&'a str, &'a str)>>(iter: T) -> Vec<String> {
    iter.map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value))).collect()
}
