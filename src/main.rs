use async_compression::tokio::bufread::GzipDecoder;
use std::process::exit;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio_tar::Archive;

use reqwest::{Client, get};

async fn run_cmd(cmd: &str, args: &[&str]) {
    let result = Command::new(cmd).args(args).output().await;

    match result {
        Ok(output) => {
            if output.status.success() {
                let result = String::from_utf8(output.stdout).expect("Error stdout");
                println!("{result}");
            } else {
                let error = String::from_utf8(output.stderr).expect("Error stderr");
                println!("{error}");
                exit(output.status.code().unwrap());
            }
        }
        Err(error) => {
            panic!("{cmd} {:?}=> {error}", args);
        }
    }
}
fn get_partition_name(device: &str, partition_number: u8) -> String {
    if device.contains("mmcblk") || device.contains("nvme") {
        return format!("{device}p{partition_number}");
    } else {
        return format!("{device}{partition_number}");
    }
}

async fn create_partitions(device: &str) -> (&'static str, &'static str) {
    run_cmd("sgdisk", &["--zap-all", device]).await;
    run_cmd("sgdisk", &[r#"-n 2:0:+512M -t 2:8300 -c 2:"boot""#, device]).await;
    run_cmd("sgdisk", &[r#"-n 3:0:0 -t 3:8300 -c 3:"root""#, device]).await;
    run_cmd("sgdisk", &["-A 2:set:2", device]).await;

    let boot_partition = get_partition_name(device, 2);
    let root_partition = get_partition_name(device, 3);

    let root_dir = "/mnt";
    let boot_dir = "/mnt/boot";

    run_cmd("mkfs.ext4", &[&boot_partition]).await;
    run_cmd("mkfs.ext4", &[&root_partition]).await;

    run_cmd("mount", &[&root_partition, root_dir]).await;
    run_cmd("mkdir", &[boot_dir]).await;
    run_cmd("mount", &[&boot_partition, boot_dir]).await;

    (root_dir, boot_dir)
}

async fn download_linux() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();

    println!("Downloading arch linux latest...");
    let mut res = client
        .get("http://os.archlinuxarm.org/os/ArchLinuxARM-aarch64-latest.tar.gz")
        .send()
        .await?;
    let mut file = File::create("archlinux.tar.gz").await?;
    while let Some(chunk) = res.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}
async fn extracting(dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("Extracting...");
    let mut file = File::open("archlinux.tar.gz").await?;
    let buf_reader = BufReader::new(file);
    let decoder = GzipDecoder::new(buf_reader);

    let mut p = Archive::new(decoder);
    p.unpack(dir).await?;
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = "/dev/sda";
    let (root_dir, boot_dir) = create_partitions(device).await;
    download_linux().await?;
    extracting(root_dir).await?;

    Ok(())
}
