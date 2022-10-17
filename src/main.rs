use anyhow::{anyhow, bail, Error};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;
use tracing_subscriber::fmt::format::FmtSpan;

use clap::Parser;
use ureq::Agent;

#[cfg_attr(target_os = "windows", path = "windows.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
mod os;

#[derive(Parser, Debug)]
#[clap(about, author)]
struct Opts {
    /// Subdomain and domain to update DNS records for
    #[clap()]
    domain: String,

    /// Run as a daemon listening for network changes
    #[clap(short, long)]
    daemon: bool,

    /// Whether to enable Cloudflare proxying for the DNS record
    #[clap(short, long)]
    proxied: bool,
    /// Explicit TTL of the DNS record
    #[clap(long)]
    ttl: Option<u32>,

    /// Cloudflare API token
    #[clap(short, long, env = "CLOUDFLARE_API_TOKEN")]
    token: String,

    /// Timeout of HTTP operation in seconds
    #[clap(long, default_value = "10")]
    timeout: u64,
    /// User agent
    #[clap(long, default_value = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))]
    user_agent: String,
}

fn main() {
    let opts = Opts::parse();
    let token = opts.token.clone();
    let daemon = opts.daemon;

    tracing_subscriber::fmt()
        .with_span_events(FmtSpan::CLOSE)
        .init();
    tracing::info!("{:?}", &opts);

    let http = ureq::builder()
        .user_agent(&opts.user_agent)
        .middleware(move |req: ureq::Request, next: ureq::MiddlewareNext<'_>| {
            next.handle(req.set("Authorization", &format!("Bearer {token}")))
        })
        .timeout(Duration::from_secs(opts.timeout))
        .build();

    let f = move || {
        (
            update(
                &http,
                &opts,
                Statics {
                    ip_type: "ipv4",
                    ip_url: "http://ip4.raftar.io",
                    record_type: "A",
                },
            )
            .ok(),
            update(
                &http,
                &opts,
                Statics {
                    ip_type: "ipv6",
                    ip_url: "http://ip6.raftar.io",
                    record_type: "AAAA",
                },
            )
            .ok(),
        )
    };

    if daemon {
        os::on_change(f).unwrap();
    } else {
        f();
    }
}

struct Statics {
    ip_type: &'static str,
    ip_url: &'static str,
    record_type: &'static str,
}

#[tracing::instrument(err, skip_all, fields(domain = opts.domain, ty.ip = statics.ip_type, ty.record = statics.record_type))]
fn update(http: &Agent, opts: &Opts, statics: Statics) -> Result<IpAddr, Error> {
    let ip = get_ip(&statics)?;
    let zone_id = get_zone_id(&opts.domain, http, opts, &statics)?;
    let mut records = get_records(&zone_id, http, opts, &statics)?;

    match records.len() {
        1 => update_record(ip, records.pop().unwrap().id, &zone_id, http, opts)?,
        0 => create_record(ip, &zone_id, http, opts, &statics)?,
        _ => bail!("multiple dns records found"),
    };
    Ok(ip)
}

#[derive(Deserialize)]
struct Res<T> {
    result: T,
}
#[derive(Deserialize, Debug)]
struct Record {
    id: String,
}

#[tracing::instrument(err, skip_all, fields(url = statics.ip_url))]
fn get_ip(statics: &Statics) -> Result<IpAddr, Error> {
    Ok(ureq::get(statics.ip_url).call()?.into_string()?.parse()?)
}

#[tracing::instrument(err, skip(http, opts, statics))]
fn get_zone_id(
    domain: &str,
    http: &Agent,
    opts: &Opts,
    statics: &Statics,
) -> Result<String, Error> {
    let mut zones = http
        .get("https://api.cloudflare.com/client/v4/zones")
        .query("name", domain)
        .call()
        .readable()?
        .into_json::<Res<Vec<Record>>>()?
        .result;

    match zones.len() {
        1 => Ok(zones.pop().unwrap().id),
        0 => match domain.split_once('.') {
            Some((_, domain)) => get_zone_id(domain, http, opts, statics),
            _ => Err(anyhow!("no zone found")),
        },
        _ => Err(anyhow!("multiple zones found")),
    }
}

#[tracing::instrument(err, skip_all)]
fn get_records(
    zone_id: &str,
    http: &Agent,
    opts: &Opts,
    statics: &Statics,
) -> Result<Vec<Record>, Error> {
    Ok(http
        .get(&format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"
        ))
        .query("name", &opts.domain)
        .query("type", statics.record_type)
        .call()
        .readable()?
        .into_json::<Res<Vec<Record>>>()?
        .result)
}

#[tracing::instrument(err, skip(zone_id, http, opts, statics), fields(proxied = opts.proxied, ttl = opts.ttl))]
fn create_record(
    ip: IpAddr,
    zone_id: &str,
    http: &Agent,
    opts: &Opts,
    statics: &Statics,
) -> Result<(), Error> {
    #[derive(Serialize)]
    struct Create<'a> {
        name: &'a str,
        #[serde(rename = "type")]
        record_type: &'static str,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        ttl: Option<u32>,
        proxied: bool,
    }

    http.post(&format!(
        "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"
    ))
    .send_json(Create {
        name: &opts.domain,
        record_type: statics.record_type,
        content: ip.to_string(),
        ttl: opts.ttl,
        proxied: opts.proxied,
    })
    .readable()?;
    Ok(())
}

#[tracing::instrument(err, skip(id, zone_id, http, opts), fields(proxied = opts.proxied, ttl = opts.ttl))]
fn update_record(
    ip: IpAddr,
    id: String,
    zone_id: &str,
    http: &Agent,
    opts: &Opts,
) -> Result<(), Error> {
    #[derive(Serialize)]
    struct Update {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        ttl: Option<u32>,
        proxied: bool,
    }

    http.patch(&format!(
        "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{id}"
    ))
    .send_json(Update {
        content: ip.to_string(),
        ttl: opts.ttl,
        proxied: opts.proxied,
    })
    .readable()?;
    Ok(())
}

trait UreqErrorExt<T> {
    fn readable(self) -> Result<T, Error>;
}
impl<T> UreqErrorExt<T> for Result<T, ureq::Error> {
    fn readable(self) -> Result<T, Error> {
        #[derive(Deserialize)]
        struct Res {
            errors: Vec<Err>,
        }
        #[derive(Deserialize)]
        struct Err {
            code: u32,
            message: String,
        }

        match self {
            Ok(t) => Ok(t),
            Err(ureq::Error::Transport(t)) => Err(anyhow!("http error: {t}")),
            Err(ureq::Error::Status(s, r)) => {
                let reasons = r
                    .into_json::<Res>()?
                    .errors
                    .into_iter()
                    .map(|e| format!("{} ({})", e.message, e.code))
                    .collect::<Vec<_>>();

                Err(anyhow!("cloudflare api error {s}: {}", reasons.join(", ")))
            }
        }
    }
}
