use configparser::ini::Ini;
use rand::Rng;
use rusoto_cloudfront::{
    CloudFront, CloudFrontClient, CreateInvalidationRequest, InvalidationBatch, Paths,
};
use rusoto_core::request::HttpClient;
use rusoto_core::{Region, RusotoError};
use rusoto_credential::ProfileProvider;
use rusoto_s3::{
    PutObjectError, PutObjectOutput, PutObjectRequest, S3Client, StreamingBody,
    S3,
};
use std::io::Write;
use std::path::Path;
use std::{env, fs, str};
use tokio::runtime::Runtime;


pub struct Config {
    pub mc_campaign_url: String,
    pub s3_bucket: String,
    pub cf_distro_id: String,
    pub region: String,
    pub profile: String,
}

impl Config {
    pub fn region(&self) -> Region {
        let region: Region = self.region.parse().unwrap();

        region
    }
}

pub fn parse_config() -> Result<Config, Box<dyn std::error::Error>> {
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


#[derive(Debug)]
pub enum S3Content {
    Img(String),
    Text(String),
}

#[derive(Debug)]
pub struct PutRequest {
    pub src: S3Content,
    pub dest: String,
    pub mime: String,
}

pub trait S3ClientExt {
    fn put_file(
        &self,
        config: &Config,
        put_request: PutRequest,
    ) -> Result<PutObjectOutput, RusotoError<PutObjectError>>;
}

impl S3ClientExt for S3Client {
    fn put_file(
        &self,
        config: &Config,
        put_request: PutRequest,
    ) -> Result<PutObjectOutput, RusotoError<PutObjectError>> {
        let (meta, body) = match &put_request.src {
            S3Content::Text(src) => {
                let contents = get_file_contents(&src);
                let meta = ::std::fs::metadata(&src).unwrap();
                let stream = ::futures::stream::once(futures::future::ready(Ok(contents.into())));
                let body = Some(StreamingBody::new(stream));

                (meta, body)
            }
            S3Content::Img(src) => {
                let contents = fs::read(&src).unwrap();
                let meta = ::std::fs::metadata(&src).unwrap();
                let body = Some(contents.into());

                (meta, body)
            }
        };

        let put_req = PutObjectRequest {
            bucket: config.s3_bucket.to_owned(),
            key: String::from(&put_request.dest),
            content_length: Some(meta.len() as i64),
            content_type: Some(put_request.mime.clone()),
            body: body,
            ..Default::default()
        };

        let result = Runtime::new().unwrap().block_on(self.put_object(put_req));

        println!(
            "Deploying {:?} to {}/{} \nResult: {:?}",
            &put_request, config.s3_bucket, put_request.dest, result
        );

        result
    }
}

pub struct SysPaths {
    pub dir: String,
    pub exe: String,
}

pub fn get_paths() -> Result<SysPaths, Box<dyn std::error::Error>> {
    let dir = env::current_dir()?.display().to_string();
    let current_exe = env::current_exe()?.display().to_string();
    let exe = Path::new(&current_exe)
        .parent()
        .unwrap()
        .display()
        .to_string();

    Ok(SysPaths { dir, exe })
}

pub struct LatestResult {
    pub html: String,
    pub link: String,
}

pub fn get_s3_client(config: &Config) -> Result<S3Client, Box<dyn std::error::Error>> {
    let provider = ProfileProvider::with_default_credentials(&config.profile);
    let client = S3Client::new_with(
        HttpClient::new().unwrap(),
        provider.unwrap(),
        config.region(),
    );

    Ok(client)
}

pub fn get_file_contents(filename: &str) -> String {
    let contents = fs::read_to_string(filename).expect(&format!(
        "Something went wrong reading the file: {}",
        &filename
    ));

    contents
}

pub fn get_file_handle(filename: &str) -> Result<std::fs::File, Box<dyn std::error::Error>> {
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

pub fn download_image(url: &str, dst_dir: &str) -> Result<PutRequest, Box<dyn std::error::Error>> {
    let response = reqwest::blocking::get(url)?;

    let fname = response
        .url()
        .path_segments()
        .and_then(|segments| segments.last())
        .and_then(|name| if name.is_empty() { None } else { Some(name) })
        .unwrap_or("tmp.bin");

    println!("file to download: '{}'", &fname);

    let paths = get_paths()?;

    let output_dir = format!("{}/{}", &paths.exe, &dst_dir);

    fs::create_dir_all(&output_dir)?;

    println!("will be located under: '{}'", &output_dir);

    let output_fname = format!("{}/{}", &output_dir, &fname);

    println!("Creating the file {}", &output_fname);

    let mut dest = fs::File::create(&output_fname)?;

    let s3_dst = format!("{}/{}", &dst_dir, &fname);

    let content = response.bytes()?;

    dest.write_all(&content)?;

    let parts: Vec<&str> = output_fname.split('.').collect();
    let mime = match parts.last() {
        Some(v) => match *v {
            "png" => mime::IMAGE_PNG,
            "jpg" => mime::IMAGE_JPEG,
            "jpeg" => mime::IMAGE_JPEG,
            "bmp" => mime::IMAGE_BMP,
            "gif" => mime::IMAGE_GIF,
            &_ => mime::TEXT_PLAIN,
        },
        None => mime::TEXT_PLAIN,
    };

    Ok(PutRequest {
        src: S3Content::Img(output_fname),
        dest: String::from(&s3_dst),
        mime: format!("{}/{}", mime.type_(), mime.subtype()),
    })
}

pub fn create_cloudfront_invalidation(config: &Config, items: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let provider = ProfileProvider::with_default_credentials(&config.profile);
    let client = CloudFrontClient::new_with(
        HttpClient::new().unwrap(),
        provider.unwrap(),
        config.region(),
    );

    let caller_reference: u8 = rand::thread_rng().gen();

    let paths = Paths {
        items: Some(items),
        quantity: 4,
    };

    let batch = InvalidationBatch {
        caller_reference: caller_reference.to_string(),
        paths: paths,
    };

    let request = CreateInvalidationRequest {
        distribution_id: config.cf_distro_id.to_string(),
        invalidation_batch: batch,
    };

    let result = Runtime::new()
        .unwrap()
        .block_on(client.create_invalidation(request));

    println!("{:?}", result);

    Ok(())
}
