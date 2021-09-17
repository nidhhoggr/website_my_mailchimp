use futures::TryStreamExt;
use rusoto_core::RusotoError;
use rusoto_s3::{GetObjectError, GetObjectRequest, S3Client, S3};
use scraper::{Html, Selector};
use std::io::Write;
use std::path::Path;
use std::{fs, panic::panic_any, str};
use tokio::runtime::Runtime;
use url::Url;
use website_my_mailchimp::{Config, LatestResult, PutRequest, S3ClientExt, S3Content};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = website_my_mailchimp::parse_config()?;

    let paths = website_my_mailchimp::get_paths()?;

    assert!(
        Path::new(&format!("{}/config.ini", &paths.dir)).exists(),
        "config.ini must exist at {}",
        &paths.dir
    );
    assert!(
        Path::new(&format!("{}/templates/top.html", &paths.dir)).exists(),
        "top.html must exist at {}/templates/",
        &paths.dir
    );
    assert!(
        Path::new(&format!("{}/templates/bottom.html", &paths.dir)).exists(),
        "bottom.html exist at {}/templates/",
        &paths.dir
    );

    let latest_res: LatestResult = get_mailchimp_latest(&config.mc_campaign_url)?;

    println!("Latest link from Mailchimp: {}", latest_res.link);

    let s3_client = website_my_mailchimp::get_s3_client(&config)?;

    let s3_lockfile = get_s3_lockfile(&config, &s3_client);

    //we'll only allow NoSuchKey errors
    let s3_lockfile_contents = match s3_lockfile {
        Ok(body) => body,
        Err(error) => match error {
            RusotoError::Service(err) => match err {
                GetObjectError::NoSuchKey(error) => error,
                _ => panic_any(err),
            },
            _ => panic_any(error),
        },
    };

    println!("Latest link from S3 lockfile: {}", s3_lockfile_contents);

    if s3_lockfile_contents.ne(&latest_res.link) {
        println!("Running jobs!");
        build_index(&latest_res)?;
        let archive_html = get_mailchimp_archive(&config.mc_campaign_url)?;
        build_archive(archive_html)?;
        put_s3_builds(&config, &s3_client)?;
        invalidate_cloudfront_builds(&config)?;
        put_s3_mailchimp_images(&config, &s3_client, &latest_res.html)?;
        put_s3_lockfile(&config, &s3_client)?;
    } else {
        println!("s3 already contains latest copy");
    }

    Ok(())
}

fn get_s3_lockfile(
    config: &Config,
    client: &S3Client,
) -> Result<String, RusotoError<GetObjectError>> {
    let get_req = GetObjectRequest {
        bucket: config.s3_bucket.to_owned(),
        key: String::from("lockfile.txt"),
        ..Default::default()
    };

    let result = Runtime::new().unwrap().block_on(client.get_object(get_req));
    let stream = result?.body.unwrap();
    let mut body = Runtime::new()
        .unwrap()
        .block_on(
            stream
                .map_ok(|b| bytes::BytesMut::from(&b[..]))
                .try_concat(),
        )
        .unwrap()
        .freeze();
    let b = body.split_to(body.len());
    let string = String::from_utf8(b.to_vec()).unwrap();

    Ok(string)
}

fn get_mailchimp_latest(url: &str) -> Result<LatestResult, Box<dyn std::error::Error>> {
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

    let paths = website_my_mailchimp::get_paths()?;

    let output_dir = format!("{}/scraped", paths.exe);

    fs::create_dir_all(&output_dir)?;

    fs::write(&format!("{}/latest.html", output_dir), node.html()).expect("Unable to write file");

    fs::write(&format!("{}/lockfile.txt", output_dir), title_href).expect("Unable to write file");

    Ok(LatestResult {
        html: node.html(),
        link: title_href.to_string(),
    })
}

fn get_mailchimp_archive(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let html = reqwest::blocking::get(url)?.text()?;

    let fragment = Html::parse_fragment(html.as_str());

    let selector_items = Selector::parse("ul#archive-list").unwrap();

    let node = fragment.select(&selector_items).next().unwrap();

    let paths = website_my_mailchimp::get_paths()?;

    fs::write(&format!("{}/scraped/archive.html", paths.exe), node.html())
        .expect("Unable to write file");

    Ok(node.html())
}

fn build_index(latest: &LatestResult) -> Result<(), Box<dyn std::error::Error>> {
    let mut index_fh = website_my_mailchimp::get_file_handle("dist/index.html")?;
    let templates_top = website_my_mailchimp::get_file_contents("templates/top.html");
    let templates_bottom = website_my_mailchimp::get_file_contents("templates/bottom.html");
    write!(index_fh, "{}", templates_top)?;
    write!(index_fh, "{}", latest.html)?;
    write!(index_fh, "{}", templates_bottom)?;

    Ok(())
}

fn build_archive(html: String) -> Result<(), Box<dyn std::error::Error>> {
    let mut archive_fh = website_my_mailchimp::get_file_handle("dist/archive.html")?;
    let templates_top = website_my_mailchimp::get_file_contents("templates/top.html");
    let templates_bottom = website_my_mailchimp::get_file_contents("templates/bottom.html");
    write!(archive_fh, "{}", templates_top)?;
    write!(archive_fh, "{}", html)?;
    write!(archive_fh, "{}", templates_bottom)?;

    Ok(())
}

fn put_s3_builds(config: &Config, client: &S3Client) -> Result<(), Box<dyn std::error::Error>> {
    client.put_file(
        &config,
        PutRequest {
            src: S3Content::Text(String::from("dist/index.html")),
            dest: String::from("index.html"),
            mime: String::from("text/html"),
        },
    )?;

    client.put_file(
        &config,
        PutRequest {
            src: S3Content::Text(String::from("dist/archive.html")),
            dest: String::from("archive.html"),
            mime: String::from("text/html"),
        },
    )?;

    Ok(())
}

fn invalidate_cloudfront_builds(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let items = Vec::from([
        String::from("/index.html"),
        String::from("/archive.html"),
        String::from("/assets/js/main.js"),
        String::from("/assets/css/main.css"),
    ]);

    website_my_mailchimp::create_cloudfront_invalidation(&config, items)
}

fn put_s3_mailchimp_images(
    config: &Config,
    client: &S3Client,
    html: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let fragment = Html::parse_fragment(&html);

    let selector_items = Selector::parse("img").unwrap();

    for element in fragment.select(&selector_items) {
        let src = element.value().attr("src").unwrap();
        let url = Url::parse(src)?;
        let hostname = url.host_str();
        if hostname == Some("gallery.mailchimp.com") || hostname == Some("mcusercontent.com") {
            let put_req =
                website_my_mailchimp::download_image(&src, "dist/assets/mailchimpGallery")?;
            let _result = client.put_file(&config, put_req);
        }
    }

    Ok(())
}

fn put_s3_lockfile(config: &Config, client: &S3Client) -> Result<(), Box<dyn std::error::Error>> {
    let _result = client.put_file(
        &config,
        PutRequest {
            src: S3Content::Text(String::from("scraped/lockfile.txt")),
            dest: String::from("lockfile.txt"),
            mime: String::from("text/plain"),
        },
    );

    Ok(())
}
