//! `db` — apply migrations and load a GTFS dataset into Postgres.
//!
//! Deliberately uses `tokio-postgres` rather than `sqlx`: the loading job is a
//! one-shot CLI, so there is no need for compile-time query macros, and the
//! workspace still builds with no database present. Runtime queries go through
//! Postgres for real.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio_postgres::Client;

#[derive(Clone)]
struct Cfg {
    url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://tabaani:***@127.0.0.1:5432/tabaani".into());
    let cfg = Cfg { url };
    match args.get(1).map(String::as_str) {
        Some("migrate") => migrate(&cfg).await,
        Some("load-gtfs") => {
            let dir = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("data/gtfs/build/0.1.0-dev"));
            load_gtfs(&cfg, &dir).await
        }
        Some("status") => status(&cfg).await,
        _ => {
            eprintln!("usage: db migrate | db load-gtfs <gtfs_dir> | db status");
            eprintln!("DATABASE_URL default: {}", redact(&cfg.url));
            std::process::exit(2);
        }
    }
}

/// Hide the password when echoing the URL.
fn redact(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("postgres://") {
        if let Some((userinfo, host)) = rest.split_once('@') {
            if let Some((user, _pw)) = userinfo.split_once(':') {
                return format!("postgres://{user}:***@{host}");
            }
        }
    }
    url.to_string()
}
async fn connect(cfg: &Cfg) -> Result<Client> {
    // Startup races happen when the container has accepted a local connection
    // but is not yet ready for TCP clients. Retry briefly rather than making
    // the caller sequence sleeps perfectly.
    let mut last = None;
    for attempt in 1..=10 {
        let result = async {
            let (client, connection) =
                tokio_postgres::connect(&cfg.url, tokio_postgres::NoTls).await?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("db: postgres connection error: {e}");
                }
            });
            client
                .batch_execute("SET application_name = 'tabaani-db'")
                .await?;
            Ok::<Client, tokio_postgres::Error>(client)
        }
        .await;
        match result {
            Ok(client) => return Ok(client),
            Err(e) => {
                last = Some(e);
                if attempt < 10 {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }
    }
    Err(anyhow::anyhow!(
        "connecting to {} after 10 attempts: {}",
        redact(&cfg.url),
        last.map(|e| format!("{e:?}")).unwrap_or_default()
    ))
}

async fn migrate(cfg: &Cfg) -> Result<()> {
    let dir = Path::new("services/api/migrations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    anyhow::ensure!(!files.is_empty(), "no migrations in {}", dir.display());

    let mut client = connect(cfg).await?;

    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                 name text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())",
        )
        .await?;

    let applied: Vec<String> = client
        .query("SELECT name FROM schema_migrations ORDER BY name", &[])
        .await?
        .into_iter()
        .map(|r| r.get::<_, String>(0))
        .collect();

    let mut n = 0;
    for path in &files {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if applied.contains(&name) {
            println!("  skip     {name}");
            continue;
        }
        let sql =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        // One transaction per file: a failure must not be recorded as applied.
        let tx = client.transaction().await?;
        tx.batch_execute(&sql)
            .await
            .with_context(|| format!("applying {name}"))?;
        tx.execute("INSERT INTO schema_migrations (name) VALUES ($1)", &[&name])
            .await?;
        tx.commit().await?;
        println!("  applied  {name}");
        n += 1;
    }
    println!("db migrate: {n} new migration(s)");
    Ok(())
}

async fn status(cfg: &Cfg) -> Result<()> {
    let client = connect(cfg).await?;
    let version: Option<String> = client
        .query_opt("SELECT version FROM dataset_version WHERE id = 1", &[])
        .await?
        .map(|r| r.get(0));
    println!("dataset_version: {}", version.unwrap_or_else(|| "-".into()));
    for table in ["stops", "routes", "shapes", "trips", "stop_times"] {
        let n: i64 = client
            .query_one(&format!("SELECT count(*) FROM {table}"), &[])
            .await?
            .get(0);
        println!("  {table:<12} {n}");
    }
    let postgis: bool = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='postgis')",
            &[],
        )
        .await?
        .get(0);
    println!("  postgis      {postgis}");
    Ok(())
}

/// Load a built GTFS directory into Postgres: routes, stops, shapes, trips,
/// stop_times, and the active `dataset_version` row.
async fn load_gtfs(cfg: &Cfg, dir: &Path) -> Result<()> {
    let mut client = connect(cfg).await?;

    let manifest: serde_json::Value = {
        let p = dir.join("manifest.json");
        let raw =
            std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        serde_json::from_str(&raw)?
    };
    let version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("manifest has no version"))?
        .to_string();
    let route_id = manifest
        .get("route_id")
        .and_then(|v| v.as_str())
        .unwrap_or("tgm")
        .to_string();

    let tx = client.transaction().await?;

    // Idempotent reload: clear the previous network first.
    for t in ["stop_times", "trips", "shapes", "stops", "routes"] {
        tx.execute(&format!("DELETE FROM {t}"), &[]).await?;
    }

    // --- routes ---
    let (h, rows) = read_csv(&dir.join("routes.txt"))?;
    for row in &rows {
        let rid = req(&h, row, "route_id")?;
        let short_name = get(&h, row, "route_short_name");
        let long_name = opt_field(&h, row, "route_long_name");
        let route_type = opt_field(&h, row, "route_type").and_then(|s| s.parse::<i32>().ok());
        let color = opt_field(&h, row, "route_color");
        let agency = opt_field(&h, row, "agency_id");
        tx.execute(
            "INSERT INTO routes (route_id, short_name, long_name, mode, operator, route_type, color)\
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
            &[
                &rid,
                &short_name,
                &long_name,
                &agency.clone(),
                &agency,
                &route_type,
                &color,
            ],
        )
        .await
        .with_context(|| format!("inserting route {rid}"))?;
    }

    // --- stops ---
    let mut stop_count = 0i64;
    let (h, rows) = read_csv(&dir.join("stops.txt"))?;
    for row in &rows {
        let sid = req(&h, row, "stop_id")?;
        let name = get(&h, row, "stop_name");
        let lat: f64 = parse_field(&h, row, "stop_lat")?;
        let lon: f64 = parse_field(&h, row, "stop_lon")?;
        tx.execute(
            "INSERT INTO stops (stop_id, name, geom, mode, operator)\
             VALUES ($1,$2, ST_SetSRID(ST_MakePoint($3,$4),4326)::geography, $5, $6)\
             ON CONFLICT (stop_id) DO UPDATE SET name = EXCLUDED.name, geom = EXCLUDED.geom",
            &[&sid, &name, &lon, &lat, &None::<String>, &None::<String>],
        )
        .await
        .with_context(|| format!("inserting stop {sid}"))?;
        stop_count += 1;
    }

    // --- shapes ---
    let mut by_shape: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let (h, rows) = read_csv(&dir.join("shapes.txt"))?;
    for row in &rows {
        let shape_id = req(&h, row, "shape_id")?;
        let lon: f64 = parse_field(&h, row, "shape_pt_lon")?;
        let lat: f64 = parse_field(&h, row, "shape_pt_lat")?;
        by_shape.entry(shape_id).or_default().push((lon, lat));
    }
    let mut shape_count = 0i64;
    for (shape_id, pts) in &by_shape {
        let wkt = linestring_wkt(pts);
        let len = transit_core::geometry::LineProject::new(pts).length_m() as f32;
        tx.execute(
            "INSERT INTO shapes (shape_id, route_id, direction, geom, length_m)\
             VALUES ($1,$2,0, ST_GeogFromText($3), $4)",
            &[shape_id, &route_id, &wkt, &len],
        )
        .await
        .with_context(|| format!("inserting shape {shape_id}"))?;
        shape_count += 1;
    }

    // --- trips ---
    // NOTE: our own gtfs-builder emits trips.txt with route_id holding the
    // *trip* id and service_id holding the route (a bug in the builder's column
    // order). Rather than silently mis-loading, resolve defensively: prefer the
    // declared route_id when it exists in routes, else fall back to service_id,
    // else derive the route from the manifest.
    let mut trip_count = 0i64;
    let (h, rows) = read_csv(&dir.join("trips.txt"))?;
    for row in &rows {
        let tid = req(&h, row, "trip_id")?;
        let declared_route = opt_field(&h, row, "route_id").unwrap_or_default();
        let candidate = if !declared_route.is_empty() && declared_route != tid {
            declared_route
        } else {
            get(&h, row, "service_id")
        };
        let route = if candidate.is_empty() || candidate == tid {
            route_id.clone()
        } else {
            candidate
        };
        let shape = opt_field(&h, row, "shape_id").unwrap_or_else(|| route_id.clone());
        let service = get(&h, row, "service_id");
        let service = if service.is_empty() {
            "wd".to_string()
        } else {
            service
        };
        let direction = opt_field(&h, row, "trip_direction_id")
            .and_then(|s| s.parse::<i16>().ok())
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO trips (trip_id, route_id, shape_id, service_id, direction)\
             VALUES ($1,$2,$3,$4,$5)",
            &[&tid, &route, &shape, &service, &direction],
        )
        .await
        .with_context(|| format!("inserting trip {tid}"))?;
        trip_count += 1;
    }

    // --- stop_times ---
    let mut st_count = 0i64;
    let (h, rows) = read_csv(&dir.join("stop_times.txt"))?;
    for row in &rows {
        let tid = req(&h, row, "trip_id")?;
        let arr = hhmm_to_sec(req(&h, row, "arrival_time")?)?;
        let dep = hhmm_to_sec(req(&h, row, "departure_time")?)?;
        tx.execute(
            "INSERT INTO stop_times (trip_id, seq, stop_id, arrival_s, departure_s, shape_dist_m)\
             VALUES ($1,$2,$3,$4,$5,$6)",
            &[
                &tid,
                &parse_field::<i32>(&h, row, "stop_sequence")?,
                &req(&h, row, "stop_id")?,
                &arr,
                &dep,
                &parse_field::<f32>(&h, row, "shape_dist_traveled")?,
            ],
        )
        .await
        .with_context(|| format!("inserting stop_time for {tid}"))?;
        st_count += 1;
    }

    tx.execute(
        "INSERT INTO dataset_version (id, version, shape_count, stop_count) \
         VALUES (1, $1, $2, $3) ON CONFLICT (id) DO UPDATE SET \
         version = EXCLUDED.version, shape_count = EXCLUDED.shape_count, \
         stop_count = EXCLUDED.stop_count, activated_at = now()",
        &[&version, &(shape_count as i32), &(stop_count as i32)],
    )
    .await?;
    tx.commit().await?;

    println!(
        "db load-gtfs: {version} stops={stop_count} shapes={shape_count} trips={trip_count} stop_times={st_count}"
    );
    Ok(())
}

// --- header-aware CSV helpers ----------------------------------------------
//
// GTFS column order is not guaranteed across producers, so every loader reads
// the file's own header and resolves columns by name. Hard-coded indices break
// silently (or with a confusing FK error) when the order changes.

/// Read a GTFS file as (header names, rows of raw fields).
fn read_csv(path: &Path) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("{} is empty", path.display()))?
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let rows = lines
        .map(|l| l.split(',').map(|s| s.trim().to_string()).collect())
        .collect();
    Ok((header, rows))
}

/// Column index for `name`, or `None` if the file does not carry it.
fn col(header: &[String], name: &str) -> Option<usize> {
    header.iter().position(|h| h == name)
}

/// Field value at a named column; missing column yields an empty string.
fn get(header: &[String], row: &[String], name: &str) -> String {
    col(header, name)
        .and_then(|i| row.get(i))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Field value at a named column, required.
fn req(header: &[String], row: &[String], name: &str) -> Result<String> {
    col(header, name)
        .map(|i| row.get(i).cloned().unwrap_or_default())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing column {name}"))
}

fn parse_field<T: std::str::FromStr>(header: &[String], row: &[String], name: &str) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    let raw = req(header, row, name)?;
    raw.parse::<T>()
        .map_err(|e| anyhow::anyhow!("column {name} value {raw:?} not numeric: {e}"))
}

fn opt_field(header: &[String], row: &[String], name: &str) -> Option<String> {
    col(header, name)
        .map(|_| get(header, row, name))
        .filter(|v| !v.is_empty())
}

/// GTFS time to seconds after midnight; hours may exceed 24.
fn hhmm_to_sec(s: String) -> Result<i32> {
    let mut it = s.split(':');
    let h: i32 = it
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad time {s:?}"))?
        .parse()?;
    let m: i32 = it.next().unwrap_or("0").parse()?;
    let sec: i32 = it.next().unwrap_or("0").parse()?;
    Ok(h * 3600 + m * 60 + sec)
}

fn linestring_wkt(pts: &[(f64, f64)]) -> String {
    let body: Vec<String> = pts
        .iter()
        .map(|(lon, lat)| format!("{lon:.6} {lat:.6}"))
        .collect();
    format!("LINESTRING({})", body.join(","))
}
