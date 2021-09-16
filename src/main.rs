use scraper::{Html, Selector};
use configparser::ini::Ini;
use std::{fs,str,env};
use std::io::Write;
use std::path::Path;


use rusoto_cloudfront::{CloudFront, CloudFrontClient, CreateInvalidationRequest, InvalidationBatch, Paths};
use rusoto_s3::{S3, S3Client, GetObjectRequest, PutObjectRequest, PutObjectOutput, PutObjectError, StreamingBody};
use rusoto_core::{Region, RusotoError};
use rusoto_credential::ProfileProvider;
use rusoto_core::request::HttpClient;
use rand::Rng;

use tokio::runtime::Runtime;
use futures::{TryStreamExt};

struct LatestResult {
    html: String,
    link: String
}

struct Config {
    mc_campaign_url: String,
    s3_bucket: String,
    cf_distro_id: String,
    region: String,
    profile: String
}

impl Config {
    fn region(&self) -> Region {
        let region: Region = self.region.parse().unwrap();

        region
    }
}

struct PutRequest {
    src: String,
    dest: String,
    mime: String,
}

trait S3ClientExt {
    fn put_file(&self, config: &Config, put_request: PutRequest) -> Result<PutObjectOutput, RusotoError<PutObjectError>>;
}

impl S3ClientExt for S3Client {
    fn put_file(&self, config: &Config, put_request: PutRequest) -> Result<PutObjectOutput, RusotoError<PutObjectError>> {
        let contents = get_file_contents(&put_request.src);
        let meta = ::std::fs::metadata(&put_request.src).unwrap();
        let stream = ::futures::stream::once(futures::future::ready(Ok(contents.into())));

        let put_req = PutObjectRequest {
            bucket: config.s3_bucket.to_owned(),
            key: String::from(&put_request.dest),
            content_length: Some(meta.len() as i64),
            content_type: Some(put_request.mime),
            body: Some(StreamingBody::new(stream)),
            ..Default::default()
        };

        let result = Runtime::new().unwrap().block_on(self.put_object(put_req));

        println!("Deploying {} to {}/{} \nResult: {:?}", &put_request.src, config.s3_bucket, put_request.dest, result);

        result
    }
}

struct SysPaths {
    dir: String,
    exe: String,
}

fn get_paths() -> Result<SysPaths, Box<dyn std::error::Error>> {  
    let dir = env::current_dir()?.display().to_string();
    let current_exe = env::current_exe()?.display().to_string();
    let exe = Path::new(&current_exe).parent().unwrap().display().to_string();
    
    Ok(SysPaths {
        dir,
        exe,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    let config = parse_config()?;

    let paths = get_paths()?;

    assert!(Path::new(&format!("{}/config.ini", &paths.dir)).exists(), "config.ini must exist at {}", &paths.dir);
    assert!(Path::new(&format!("{}/templates/top.html", &paths.dir)).exists(), "top.html must exist at {}/templates/", &paths.dir);
    assert!(Path::new(&format!("{}/templates/bottom.html", &paths.dir)).exists(), "bottom.html exist at {}/templates/", &paths.dir);

    let latest_res: LatestResult = get_latest(&config.mc_campaign_url)?;

    println!("Latest link from Mailchimp: {}", latest_res.link);

    let s3_client = get_s3_client(&config)?;
    
    let s3_latest = get_s3_latest(&config, &s3_client)?;
    
    println!("Latest link from S3: {}", s3_latest);
    
    if s3_latest.ne(&latest_res.link) {
        println!("Running jobs!");
        build_index(&latest_res)?;
        let archive_html = get_archive(&config.mc_campaign_url)?;
        build_archive(archive_html)?;
        deploy_build(&config, &s3_client)?;
        create_invalidation(&config)?;
        put_s3_latest(&config, &s3_client)?;
    }
    else {
        println!("s3 already contains latest copy");
		//download_image("https://www.rust-lang.org/logos/rust-logo-512x512.png")?;
    }

    Ok(())
}

fn parse_config() -> Result<Config, Box<dyn std::error::Error>> {
	let mut config = Ini::new();
    let _map = config.load("config.ini");

	let mc_campaign_url = config.get("DEFAULT", "mc_campaign_url").unwrap();
	let s3_bucket = config.get("aws", "s3_bucket").unwrap();
	let cf_distro_id = config.get("aws", "cf_distro_id").unwrap();
	let profile = config.get("aws", "profile").unwrap();
	let region = config.get("aws", "region").unwrap();
    
    Ok(Config {
        mc_campaign_url,
        s3_bucket,
        cf_distro_id,
        region,
        profile,
    })
}

fn build_index(latest: &LatestResult) -> Result<(), Box<dyn std::error::Error>> {
    let mut index_fh = get_file_handle("dist/index.html")?;
    let templates_top = get_file_contents("templates/top.html");
    let templates_bottom = get_file_contents("templates/bottom.html");
    write!(index_fh, "{}", templates_top)?;
    write!(index_fh, "{}", latest.html)?;
    write!(index_fh, "{}", templates_bottom)?;

    Ok(())
}

fn build_archive(html: String) -> Result<(), Box<dyn std::error::Error>> {
    let mut archive_fh = get_file_handle("dist/archive.html")?;
    let templates_top = get_file_contents("templates/top.html");
    let templates_bottom = get_file_contents("templates/bottom.html");
    write!(archive_fh, "{}", templates_top)?;
    write!(archive_fh, "{}", html)?;
    write!(archive_fh, "{}", templates_bottom)?;

    Ok(())
}

fn get_file_contents(filename: &str) -> String {
    let contents = fs::read_to_string(filename)
        .expect("Something went wrong reading the file");

    contents
}

fn get_file_handle(filename: &str) -> Result<std::fs::File, Box<dyn std::error::Error>> {

    let paths = get_paths()?;

    println!("Getting file handle for: {}", filename);

    let path = Path::new(filename);
        
    let object_prefix = path.parent().unwrap().to_str().unwrap();

    let fname = path.file_name().unwrap().to_str().unwrap();

    let output_dir = format!("{}/{}/", &paths.exe, &object_prefix);

    println!("will be located under: '{}'", &output_dir);

    fs::create_dir_all(&output_dir)?;

    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(format!("{}/{}", &output_dir, &fname))?;

    Ok(file)
}


fn get_latest(url: &str) -> Result<LatestResult, Box<dyn std::error::Error>> {

    let resp = reqwest::blocking::get(url)?;
    assert!(resp.status().is_success());
    let html = resp.text()?;
    let fragment = Html::parse_fragment(html.as_str());
    
    let selector_items = Selector::parse(".campaign a").unwrap();
    
    let node = fragment.select(&selector_items).next().unwrap();

    let title_text = node.text().nth(0).unwrap();
    let title_href = node.value().attr("href").unwrap();

    println!("links: {} {}", title_text, title_href);

    let html = reqwest::blocking::get(title_href)?.text()?;

    let fragment = Html::parse_fragment(html.as_str());

    let selector_items = Selector::parse("table").unwrap();

    let node = fragment.select(&selector_items).next().unwrap();
   
    let paths = get_paths()?;

    let output_dir = format!("{}/scraped", paths.exe);

    fs::create_dir_all(&output_dir)?;

    fs::write(&format!("{}/latest.html", output_dir), node.html()).expect("Unable to write file");
    fs::write(&format!("{}/latest.txt", output_dir), title_href).expect("Unable to write file");

    Ok(LatestResult{
        html: node.html(),
        link: title_href.to_string()
    })
}

fn download_image(url: &str) -> Result<(), Box<dyn std::error::Error>> {

    let response = reqwest::blocking::get(url)?;

    let mut dest = {
    
        let fname = response
            .url()
            .path_segments()
            .and_then(|segments| segments.last())
            .and_then(|name| if name.is_empty() { None } else { Some(name) })
            .unwrap_or("tmp.bin");
            
		println!("file to download: '{}'", fname);

        let paths = get_paths()?;

		let output_dir = format!("{}/{}", &paths.exe, "dist/assets");

		fs::create_dir_all(output_dir.clone())?;

		println!("will be located under: '{}'", output_dir.clone());

		let output_fname = format!("{}/{}", output_dir, fname);
		
		println!("Creating the file {}", output_fname);

		fs::File::create(output_fname)?
    };
    
	let content =  response.bytes()?;

	dest.write_all(&content)?;

	Ok(())
}

fn get_archive(url: &str) -> Result<String, Box<dyn std::error::Error>> {

    let html = reqwest::blocking::get(url)?.text()?;

    let fragment = Html::parse_fragment(html.as_str());

    let selector_items = Selector::parse("ul#archive-list").unwrap();
    
    let node = fragment.select(&selector_items).next().unwrap();

    let paths = get_paths()?;

    fs::write(&format!("{}/scraped/archive.html", paths.exe), node.html()).expect("Unable to write file");

    Ok(node.html())
}

fn get_s3_client(config: &Config) -> Result<S3Client, Box<dyn std::error::Error>> {

    let provider = ProfileProvider::with_default_credentials(&config.profile);
    let client = S3Client::new_with(
        HttpClient::new().unwrap(),
        provider.unwrap(),
        config.region(),
    );

    Ok(client)
}

fn get_s3_latest(config: &Config, client: &S3Client) -> Result<String, Box<dyn std::error::Error>> {

    let get_req = GetObjectRequest {
        bucket: config.s3_bucket.to_owned(),
        key: String::from("latest.txt"),
        ..Default::default()
    };

    let result = Runtime::new().unwrap().block_on(client.get_object(get_req));
    let stream = result?.body.unwrap();
    let mut body = Runtime::new().unwrap().block_on(stream.map_ok(|b| bytes::BytesMut::from(&b[..])).try_concat()).unwrap().freeze();
    let b = body.split_to(body.len());
    let string = String::from_utf8(b.to_vec())?;

    Ok(string)
}


fn put_s3_latest(config: &Config, client: &S3Client) -> Result<(), Box<dyn std::error::Error>> {
    
    let _result = client.put_file(&config, PutRequest{
        src: String::from("scraped/latest.txt"),
        dest: String::from("latest.txt"),
        mime: String::from("text/plain"),
    });

    Ok(())
}

fn deploy_build(config: &Config, client: &S3Client) -> Result<(), Box<dyn std::error::Error>> {

    let _result = client.put_file(&config, PutRequest{
        src: String::from("dist/index.html"),
        dest: String::from("index.html"),
        mime: String::from("text/html"),
    });

    let _result = client.put_file(&config, PutRequest{
        src: String::from("dist/archive.html"),
        dest: String::from("archive.html"),
        mime: String::from("text/html"),
    });

    Ok(())
}

fn create_invalidation(config: &Config) -> Result<(), Box<dyn std::error::Error>> {

    let provider = ProfileProvider::with_default_credentials(&config.profile);
    let client = CloudFrontClient::new_with(
        HttpClient::new().unwrap(),
        provider.unwrap(),
        config.region(),
    );

    let caller_reference: u8 = rand::thread_rng().gen();

    let items = Vec::from([
        String::from("/index.html"),
        String::from("/archive.html"),
        String::from("/assets/js/main.js"),
        String::from("/assets/css/main.css")
    ]);

    let paths = Paths {
        items: Some(items),
        quantity: 4
    };

    let batch = InvalidationBatch {
        caller_reference: caller_reference.to_string(),
        paths: paths
    };

    let request = CreateInvalidationRequest {
        distribution_id: config.cf_distro_id.to_string(),
        invalidation_batch: batch
    };

    let result = Runtime::new().unwrap().block_on(client.create_invalidation(request));

    println!("{:?}", result);

    Ok(())
}
