use async_compression::tokio::bufread::GzipDecoder;
use reqwest::Client;
use std::env::args;
use std::error::Error;
use std::process::exit;
use tokio::fs::File;
use tokio::fs::OpenOptions;
use tokio::fs::create_dir_all;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio_tar::Archive;

async fn run_cmd(cmd: &str, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let result = Command::new(cmd).args(args).output().await;

    println!("{cmd} {:?}", args);
    match result {
        Ok(output) => {
            if output.status.success() {
                let result = String::from_utf8(output.stdout).expect("Error stdout");
                println!("{result}");
                Ok(result)
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
        format!("{device}p{partition_number}")
    } else {
        format!("{device}{partition_number}")
    }
}

async fn create_partitions(device: &str) -> (&str, &str, String, String) {
    println!("Clear partitions...");
    let _ = run_cmd("sgdisk", &["--zap-all", device]).await;
    println!("Create partitions...");
    let _ = run_cmd("sgdisk", &[r#"-n 2:0:+512M -t 2:8300 -c 2:"boot""#, device]).await;
    let _ = run_cmd("sgdisk", &[r#"-n 3:0:0 -t 3:8300 -c 3:"root""#, device]).await;
    let _ = run_cmd("sgdisk", &["-A 2:set:2", device]).await;

    let boot_partition = get_partition_name(device, 2);
    let root_partition = get_partition_name(device, 3);

    println!("Mount partitions...");
    let root_dir = "/mnt";
    let boot_dir = "/mnt/boot";

    let _ = run_cmd("mkfs.ext4", &[&boot_partition]).await;
    let _ = run_cmd("mkfs.ext4", &[&root_partition]).await;

    let _ = run_cmd("mount", &[&root_partition, root_dir]).await;
    let _ = run_cmd("mkdir", &[boot_dir]).await;
    let _ = run_cmd("mount", &[&boot_partition, boot_dir]).await;

    (root_dir, boot_dir, root_partition, boot_partition)
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
async fn extracting_tar(dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("Extracting tar file...");
    let file = File::open("archlinux.tar.gz").await?;
    let buf_reader = BufReader::new(file);
    let decoder = GzipDecoder::new(buf_reader);
    Archive::new(decoder).unpack(dir).await?;
    Ok(())
}

fn extract_uuid(line: &str) -> Option<String> {
    if let Some(start) = line.find("UUID=\"") {
        if let Some(end) = line[start + 6..].find("\"") {
            return Some(line[start + 6..start + 6 + end].to_string());
        }
    }
    None
}
async fn modify_fstab(
    root_part: String,
    boot_part: String,
) -> Result<String, Box<dyn std::error::Error>> {
    println!("Modify fstab...");

    let out = run_cmd("blkid", &[&root_part, &boot_part]).await?;
    let v: Vec<&str> = out.lines().collect();
    if v.len() < 2 {
        return Err("Not found both partitions".into());
    }

    let mut file = OpenOptions::new()
        .append(true)
        .open("/mnt/etc/fstab")
        .await?;
    let root_uuid;

    if v[0].contains(&root_part) {
        root_uuid = extract_uuid(v[0]).expect("uuid not found");
        let out = format!("UUID={root_uuid} / ext4 defaults 0 1\n");
        file.write_all(out.as_bytes()).await?;
    } else {
        return Err("No root partition".into());
    }
    if v[1].contains(&boot_part) {
        let uuid = extract_uuid(v[1]).expect("uuid not found");
        let out = format!("UUID={uuid} /boot ext4 defaults 0 2");
        file.write_all(out.as_bytes()).await?;
    } else {
        return Err("No boot partition".into());
    }

    file.flush().await?;

    Ok(root_uuid)
}

async fn create_extlinux(root_uuid: &str) -> Result<(), Box<dyn Error>> {
    println!("Create extlinux...");
    let out = format! {
"DEFAULT arch
MENU TITLE Boot Menu
PROMPT 0
TIMEOUT 50\n
LABEL arch
MENU LABEL Arch Linux ARM
LINUX /Image
INITRD /initramfs-linux.img
FDT /dtbs/rockchip/rk3399-pinebook-pro.dtb
APPEND root=UUID={root_uuid} rw\n
LABEL arch-fallback
MENU LABEL Arch Linux ARM with fallback initramfs
LINUX /Image
INITRD /initramfs-linux-fallback.img
FDT /dtbs/rockchip/rk3399-pinebook-pro.dtb
APPEND root=UUID={root_uuid} rw"};

    create_dir_all("/mnt/boot/extlinux/extlinux").await?;
    File::create("/mnt/boot/extlinux/extlinux.conf")
        .await?
        .write_all(out.as_bytes())
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = args().collect();
    if args.len() < 2 {
        return Err("Incorrect arguments - No device found".into());
    }

    let device = args[1].as_str();
    let (root_dir, _boot_dir, _root_part, _boot_part) = create_partitions(device).await;
    download_linux().await?;
    extracting_tar(root_dir).await?;
    let root_uuid = modify_fstab("/dev/sdc3".to_string(), "/dev/sdc2".to_string()).await?;
    create_extlinux(&root_uuid).await?;

    Ok(())
}
