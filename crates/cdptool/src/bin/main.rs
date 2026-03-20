use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::Path;

use cdptool::cdp::{self as cdpmod, CdpDocument, CdpTag};
use cdptool::extract;
use cdptool::lzss;

fn cmd_extract(cdp_path: &str, out_dir: &str) -> Result<()> {
    let data = fs::read(cdp_path).with_context(|| format!("reading {cdp_path}"))?;
    let doc = cdpmod::parse(&data)?;
    let entries = extract::extract_all(&doc)?;

    let mut ok = 0usize;
    let mut fail = 0usize;

    for entry in &entries {
        let out_path = Path::new(out_dir).join(&entry.path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::File::create(&out_path) {
            Ok(mut f) => {
                f.write_all(&entry.data)?;
                ok += 1;
                println!("OK:   {} ({} bytes)", entry.path, entry.data.len());
            }
            Err(e) => {
                fail += 1;
                println!("FAIL: {} ({e})", entry.path);
            }
        }
    }

    println!("\n{ok}/{} extracted to {out_dir}/", entries.len());
    if fail > 0 {
        bail!("{fail} file(s) failed");
    }
    Ok(())
}

fn cmd_info(cdp_path: &str) -> Result<()> {
    let data = fs::read(cdp_path).with_context(|| format!("reading {cdp_path}"))?;
    let doc = cdpmod::parse(&data)?;

    println!("=== {cdp_path} ===");
    println!(
        "Version: {}  Reserved: {}  Size: {} bytes\n",
        doc.version,
        doc.reserved,
        data.len()
    );
    for tag in &doc.tags {
        print!("{}", tag.display(0));
    }
    Ok(())
}

fn cmd_create(cdp_path: &str, dir: &str, args: &[String]) -> Result<()> {
    let mut username = "unknown".to_string();
    let mut level: u8 = 9;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--username" => {
                username = args
                    .get(i + 1)
                    .context("--username requires a value")?
                    .clone();
                i += 2;
            }
            "--level" => {
                level = args
                    .get(i + 1)
                    .context("--level requires a value")?
                    .parse()
                    .context("--level must be 0–15")?;
                i += 2;
            }
            other => bail!("unknown flag: {other}"),
        }
    }

    let dir_path = Path::new(dir);
    if !dir_path.is_dir() {
        bail!("{dir} is not a directory");
    }

    let mut file_tags: Vec<CdpTag> = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let raw_data = fs::read(entry.path())?;
        let uncomp_size = raw_data.len() as u32;

        let compressed = lzss::compress(&raw_data, 2, level)?;

        let mut blob = uncomp_size.to_le_bytes().to_vec();
        blob.extend_from_slice(&compressed);

        println!(
            "  {name}: {} -> {} bytes ({:.0}%)",
            raw_data.len(),
            blob.len(),
            blob.len() as f64 / raw_data.len().max(1) as f64 * 100.0
        );

        file_tags.push(CdpTag::Binary { name, data: blob });
    }

    let asset_children = vec![
        CdpTag::String {
            name: "username".into(),
            value: username,
        },
        CdpTag::String {
            name: "kind".into(),
            value: "archive".into(),
        },
        CdpTag::String {
            name: "compression".into(),
            value: "LZSS".into(),
        },
        CdpTag::Container {
            name: "files".into(),
            children: file_tags,
        },
    ];

    let doc = CdpDocument {
        version: 1,
        reserved: 0,
        tags: vec![
            CdpTag::Container {
                name: "assets".into(),
                children: vec![CdpTag::Container {
                    name: "asset".into(),
                    children: asset_children,
                }],
            },
            CdpTag::Container {
                name: "contents-table".into(),
                children: vec![],
            },
            CdpTag::Container {
                name: "kuid-table".into(),
                children: vec![],
            },
            CdpTag::Container {
                name: "obsolete-table".into(),
                children: vec![],
            },
            CdpTag::String {
                name: "kind".into(),
                value: "archive".into(),
            },
            CdpTag::Integer {
                name: "package-version".into(),
                values: vec![1],
            },
            CdpTag::String {
                name: "username".into(),
                value: "cdp-tool".into(),
            },
        ],
    };

    let serialized = cdpmod::serialize(&doc);
    fs::write(cdp_path, &serialized)?;

    println!(
        "\nCreated {cdp_path} ({} bytes, {} files)",
        serialized.len(),
        entries.len()
    );
    Ok(())
}

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  cdptool extract <file.cdp> <outdir>     Extract files from CDP archive");
    eprintln!("  cdptool info <file.cdp>                  Show CDP structure");
    eprintln!("  cdptool create <out.cdp> <dir> [flags]   Create CDP from directory");
    eprintln!();
    eprintln!("Create flags:");
    eprintln!("  --username <name>    Set username (default: unknown)");
    eprintln!("  --level <0-15>       Compression level (default: 9, 0=uncompressed)");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }

    let result = match args[1].as_str() {
        "extract" if args.len() >= 4 => cmd_extract(&args[2], &args[3]),
        "info" if args.len() >= 3 => cmd_info(&args[2]),
        "create" if args.len() >= 4 => cmd_create(&args[2], &args[3], &args[4..]),
        other if other.ends_with(".cdp") => {
            let out_dir = args.get(2).map(|s| s.as_str()).unwrap_or("out");
            cmd_extract(other, out_dir)
        }
        _ => usage(),
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
