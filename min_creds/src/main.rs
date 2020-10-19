#[macro_use]
extern crate log;

use std::collections::{HashMap, HashSet, VecDeque};
use std::iter::FromIterator;
use std::ops::Add;
use std::sync::Arc;

use actix::{Actor, AsyncContext, clock, Context};
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, middleware, web};
use actix_web::rt::Arbiter;
use actix_web::rt::time::delay_for;
use actix_web_httpauth::extractors::AuthenticationError;
use actix_web_httpauth::extractors::bearer::Config;
use actix_web_httpauth::middleware::HttpAuthentication;
use argh::FromArgs;
use chrono::prelude::*;
use eyre::{Result, WrapErr};
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

mod config;

#[derive(FromArgs, PartialEq, Debug)]
/// main arguments
struct MainArgs {
    #[argh(positional, description = "config file")]
    config_file: String
}

struct Cred {
    pub user: String,
    pub password: String,
}

impl From<Lease> for Cred {
    fn from(lease: Lease) -> Self {
        Self {
            user: lease.user,
            password: lease.password,
        }
    }
}

#[derive(PartialEq, Eq, Hash, Debug, Clone)]
struct LeaseId(String);

impl From<String> for LeaseId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl LeaseId {
    fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

#[derive(Clone)]
struct Lease {
    pub id: LeaseId,
    pub user: String,
    pub password: String,
    pub expires_on: DateTime<Utc>,
    pub created_on: DateTime<Utc>,
    pub client_name: String,
}

impl Lease {
    fn from_cred(cred: Cred, expiration_duration: chrono::Duration, client_name: &str) -> Lease {
        let now = Utc::now();
        Lease {
            id: LeaseId::new(),
            user: cred.user,
            password: cred.password,
            expires_on: now.add(expiration_duration),
            created_on: now,
            client_name: client_name.to_string(),
        }
    }
}

#[derive(Eq, PartialEq, Hash, Debug)]
struct ServiceName(String);

impl From<String> for ServiceName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

struct LeaseRelease {
    pub duration: chrono::Duration,
    pub client_name: String,
}

struct Service {
    pub expires_in: chrono::Duration,
    pub leases: HashMap<LeaseId, Lease>,
    pub available_creds: VecDeque<Cred>,
}


impl Service {
    fn clear_expired_leases(&mut self) -> usize {
        let now = Utc::now();
        let mut n_expired = 0_usize;

        let mut leases_to_remove = vec![];
        for lease_id in self.leases.keys() {
            if self.leases[lease_id].expires_on < now {
                leases_to_remove.push(lease_id.clone());
            }
        }
        for lease_id in leases_to_remove {
            if let Some(lease) = self.leases.remove(&lease_id) {
                self.available_creds.push_back(lease.into());
                n_expired += 1
            }
        }
        n_expired
    }

    fn get_lease(&mut self, client_name: &str) -> Option<Lease> {
        self.clear_expired_leases();
        if let Some(cred) = self.available_creds.pop_front() {
            let lease = Lease::from_cred(cred, self.expires_in, client_name);
            self.leases.insert(lease.id.clone(), lease.clone());
            Some(lease)
        } else {
            None
        }
    }

    fn release(&mut self, lease_id: &LeaseId) -> Option<LeaseRelease> {
        if let Some(lease) = self.leases.remove(lease_id) {
            let lr = LeaseRelease {
                duration: Utc::now() - lease.created_on,
                client_name: lease.client_name.clone(),
            };
            self.available_creds.push_back(lease.into());
            Some(lr)
        } else {
            None
        }
    }

    fn leases_available(&self) -> usize {
        self.available_creds.len()
    }

    fn leases_in_use(&self) -> usize {
        self.leases.len()
    }
}

struct Cleaner {
    pub services: Arc<HashMap<ServiceName, RwLock<Service>>>
}

impl Actor for Cleaner {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(clock::Duration::new(3, 0), |this, _ctx| {
            Arbiter::spawn(Cleaner::clean(this.services.clone()));
        });
    }
}

impl Cleaner {
    async fn clean(services: Arc<HashMap<ServiceName, RwLock<Service>>>) {
        for (service_name, service) in services.iter() {
            let mut locked = service.write().await;
            let n_expired = locked.clear_expired_leases();
            if n_expired > 0 {
                info!("Cleared {} expired leases for service {}", n_expired, service_name.0)
            }
        }
    }
}

struct AppState {
    pub services: Arc<HashMap<ServiceName, RwLock<Service>>>,
    pub access_tokens: HashSet<String>,
}

impl AppState {
    fn from_cfg(cfg: &config::Config) -> Self {
        Self {
            services: Arc::new(
                cfg.services.iter().map(|(s_name, service)| {
                    let available_creds = service.credentials.iter().flat_map(|c| {
                        (0..(c.num_concurrent)).map(|_| {
                            Cred {
                                user: c.user.clone(),
                                password: c.password.clone(),
                            }
                        }).collect::<Vec<_>>()
                    }).collect::<VecDeque<_>>();

                    let s = Service {
                        expires_in: chrono::Duration::seconds(service.lease_timeout_secs as i64),
                        leases: HashMap::default(),
                        available_creds,
                    };
                    (ServiceName::from(s_name.clone()), RwLock::new(s))
                }).collect::<HashMap<_, _>>()
            ),

            access_tokens: HashSet::from_iter(cfg.access_tokens.iter().cloned()),
        }
    }
}


#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();
    println!("{} (v{})", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));

    let main_args: MainArgs = argh::from_env();
    let cfg = config::read_config(main_args.config_file)?;
    let app_state = web::Data::new(AppState::from_cfg(&cfg));

    Cleaner { services: app_state.services.clone() }.start();

    let web_path = cfg.web_path.clone();
    info!("Starting webserver on {} using path {}", cfg.listen_on, cfg.web_path);
    let server = HttpServer::new(move || {
        let auth = HttpAuthentication::bearer(|req, creds| async move {
            let config = req
                .app_data::<Config>()
                .cloned()
                .unwrap_or_else(Default::default);

            let app_state = req.app_data::<web::Data<AppState>>()
                .expect("could not access app_state");
            let token = creds.token().to_string();
            if app_state.access_tokens.contains(&token) {
                Ok(req)
            } else {
                Err(AuthenticationError::from(config).into())
            }
        });
        App::new()
            .app_data(app_state.clone())
            .wrap(middleware::DefaultHeaders::new().header(
                // on auth-requiring routes the headers are only visible after successful auth
                "Server",
                format!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION")),
            ))
            .wrap(middleware::Logger::default())
            .route(&web_path, web::get().to(overview))
            .service(
                web::scope(&web_path)
                    .wrap(auth)
                    .route("/get", web::post().to(get_lease))
                    .route("/release", web::post().to(clear_lease))
            )
    });

    if let Some(ssl_config) = cfg.ssl {
        let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())
            .wrap_err_with(|| "failed to create ssl builder")?;
        builder.set_private_key_file(ssl_config.private_key_pem_file, SslFiletype::PEM)
            .wrap_err_with(|| "reading SSL key failed")?;
        builder.set_certificate_chain_file(ssl_config.certificate_chain_file)
            .wrap_err_with(|| "reading certificate failed")?;

        server.bind_openssl(cfg.listen_on, builder)?
            .run()
            .await
    } else {
        server.bind(cfg.listen_on)?
            .run()
            .await
    }.wrap_err_with(|| "failed to run webserver")
}

#[derive(Serialize)]
struct ServiceOverview {
    pub leases_in_use: usize,
    pub leases_available: usize,
}

#[derive(Serialize)]
struct Overview {
    pub services: HashMap<String, ServiceOverview>
}

async fn overview(app_state: web::Data<AppState>) -> actix_web::Result<HttpResponse> {
    let mut overview = Overview { services: Default::default() };
    for (service_name, service) in app_state.services.iter() {
        let locked_service = service.read().await;
        let s_overview = ServiceOverview {
            leases_available: locked_service.leases_available(),
            leases_in_use: locked_service.leases_in_use(),
        };
        overview.services.insert(service_name.0.clone(), s_overview);
    }
    Ok(HttpResponse::Ok().json(overview))
}

#[derive(Serialize)]
struct ErrorResponse {
    pub message: String
}

#[derive(Deserialize)]
struct GetLeaseRequest {
    pub service: String
}

#[derive(Serialize)]
struct GetLeaseResponse {
    pub lease: String,
    pub user: String,
    pub password: String,
    pub expires_on: String,
}

async fn get_lease(request: HttpRequest, app_state: web::Data<AppState>, get_lease: web::Json<GetLeaseRequest>) -> actix_web::Result<HttpResponse> {
    let wait_start = Utc::now();

    let useragent = if let Some(ua_header) = request.headers().get("User-Agent") {
        match ua_header.to_str() {
            Ok(v) => Some(v.to_string()),
            Err(_) => None
        }
    } else {
        None
    }.unwrap_or_else(|| "<empty user-agent>".to_string());

    let service_name = ServiceName::from(get_lease.service.clone());
    loop {
        if let Some(service) = app_state.services.get(&service_name) {
            let mut locked_service = service.write().await;
            if let Some(lease) = locked_service.get_lease(&useragent) {
                let wait_millis = (Utc::now() - wait_start).num_milliseconds().abs() as f64 / 1000.0;
                if wait_millis > 10.0 {
                    warn!("client '{}' had to wait {:.3} seconds to obtain credential for {}",
                          lease.client_name, wait_millis, service_name.0);
                }

                return Ok(HttpResponse::Ok().json(GetLeaseResponse {
                    lease: lease.id.0,
                    user: lease.user,
                    password: lease.password,
                    expires_on: lease.expires_on.to_rfc3339(),
                }));
            }
        } else {
            warn!("credential request for unknown service '{}' from client {} received",
                  get_lease.service, useragent);
            return Ok(
                HttpResponse::NotFound()
                    .json(ErrorResponse {
                        message: format!("unknown service \"{}\"", get_lease.service)
                    })
            );
        }

        delay_for(clock::Duration::from_millis(300)).await
    }
}

#[derive(Deserialize)]
struct ClearLeaseRequest {
    pub lease: String
}

#[derive(Serialize)]
struct ClearLeaseResponse {}

async fn clear_lease(app_state: web::Data<AppState>, clear_lease_req: web::Json<ClearLeaseRequest>) -> actix_web::Result<HttpResponse> {
    let lease_id = LeaseId::from(clear_lease_req.lease.clone());
    for (service_name, service) in app_state.services.iter() {
        let mut locked_service = service.write().await;
        if let Some(release) = locked_service.release(&lease_id) {
            info!("credential for service {} was in use for {:.3} secs by client '{}'",
                  service_name.0, release.duration.num_milliseconds() as f64 / 1000.0, release.client_name);
            break;
        }
    }
    Ok(HttpResponse::Ok().json(ClearLeaseResponse {}))
}
