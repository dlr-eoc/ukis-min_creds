#[macro_use]
extern crate log;

use std::collections::{HashMap, HashSet, VecDeque};
use std::iter::FromIterator;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use actix::{Actor, AsyncContext, clock, Context};
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, middleware, Responder, web};
use actix_web::rt::Arbiter;
use actix_web::web::Bytes;
use actix_web_httpauth::extractors::AuthenticationError;
use actix_web_httpauth::extractors::bearer::Config;
use actix_web_httpauth::middleware::HttpAuthentication;
use argh::FromArgs;
use chrono::prelude::*;
use eyre::{Result, WrapErr};
use futures::Stream;
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use crate::service::{Lease, LeaseRelease, Service};
use crate::service::{Cred, LeaseId, ServiceName};

mod config;
mod service;

#[derive(FromArgs, PartialEq, Debug)]
/// main arguments
struct MainArgs {
    #[argh(positional, description = "config file")]
    config_file: String
}

struct Cleaner {
    pub services: Arc<HashMap<ServiceName, RwLock<ServiceBroker>>>
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
    async fn clean(services: Arc<HashMap<ServiceName, RwLock<ServiceBroker>>>) {
        for (service_name, broker_rw) in services.iter() {
            let mut broker = broker_rw.write().await;
            let n_expired = broker.clean().await;
            if n_expired > 0 {
                info!("Cleared {} expired leases for service {}", n_expired, service_name.0)
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct PersistentLease {
    pub lease_id: service::LeaseId,
    pub expiration: service::Expiration,
    pub client_name: String,
    pub cred_hash: String,
}


struct ServiceWaiter {
    sender: mpsc::Sender<Lease>,
    client_name: String,
}

struct LeaseReceiver(mpsc::Receiver<Lease>);

impl Stream for LeaseReceiver {
    type Item = Result<Bytes, actix_web::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.0).poll_next(cx) {
            Poll::Ready(Some(lease)) => {
                let response = GetLeaseResponse::from(lease);
                let bytes = Bytes::from(serde_json::to_string(&response).unwrap());
                Poll::Ready(Some(Ok(bytes)))
            },
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending
        }
    }
}

struct ServiceBroker {
    service: Service,

    /// arc to get the number of waiting tasks by fetching the
    /// Arc::strong_count()
    waiters: VecDeque<ServiceWaiter>,
}

impl ServiceBroker {
    async fn obtain_lease(&mut self, client_name: String) -> LeaseReceiver {
        let (mut sender, receiver) = mpsc::channel(1);

        let lease_opt = self.service.get_lease(&client_name);

        if let Some(lease) = lease_opt {
            let lease_id = lease.id.clone();
            if sender.try_send(lease).is_err() {
                self.service.release(&lease_id);
            }
        } else {
            self.waiters.push_back(ServiceWaiter {
                sender,
                client_name,
            });
        }
        LeaseReceiver(receiver)
    }

    async fn serve_waiters(&mut self) {
        while let Some(next_waiter) = self.waiters.front() {
            if let Some(lease) = self.service.get_lease(&next_waiter.client_name) {
                let lease_id = lease.id.clone();
                if let Some(mut waiter) = self.waiters.pop_front() {
                    if waiter.sender.try_send(lease).is_err() {
                        // receiver stopped waiting for a credential
                        self.service.release(&lease_id);
                    }
                } else {
                    self.service.release(&lease_id);
                }
            } else {
                break;
            }
        }
    }

    async fn clean(&mut self) -> usize {
        let n_cleared = self.service.clear_expired_leases();
        self.serve_waiters().await;
        n_cleared
    }

    async fn release(&mut self, lease_id: LeaseId) -> Option<LeaseRelease> {
        let lr = self.service.release(&lease_id);
        self.serve_waiters().await;
        lr
    }
}

impl From<Service> for ServiceBroker {
    fn from(s: Service) -> Self {
        Self {
            service: s,
            waiters: VecDeque::new(),
        }
    }
}

struct AppState {
    pub services: Arc<HashMap<ServiceName, RwLock<ServiceBroker>>>,
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
                    (s_name.clone().into(), RwLock::new(s.into()))
                }).collect::<HashMap<_, _>>()
            ),

            access_tokens: HashSet::from_iter(cfg.access_tokens.iter().cloned()),
        }
    }

    async fn persist_leases(&self, leases_path: &Path) -> Result<()> {
        let mut leases_file = File::create(leases_path).await?;

        let mut leases = HashMap::new();
        for (service_name, broker_rw) in self.services.iter() {
            let broker = broker_rw.read().await;

            let mut persistent_leases = vec![];
            for (lease_id, lease) in broker.service.leases.iter() {
                let persistent_lease = PersistentLease {
                    lease_id: lease_id.clone(),
                    expiration: lease.expiration.clone(),
                    client_name: lease.client_name.clone(),
                    cred_hash: Cred::from(lease.clone()).cred_hash(),
                };
                persistent_leases.push(persistent_lease);
            }

            if !persistent_leases.is_empty() {
                leases.insert(service_name.clone(), persistent_leases);
            }
        }
        let data = serde_yaml::to_string(&leases)?;
        leases_file.write_all(&data.as_bytes()).await
            .wrap_err_with(|| "could not write to leases file")
    }

    async fn load_persistent_leases(&mut self, leases_path: &Path) -> Result<()> {
        let mut leases_file = File::open(leases_path).await?;

        let mut buf: Vec<u8> = vec![];
        leases_file.read_to_end(&mut buf).await?;

        let persistent_leases: HashMap<ServiceName, Vec<PersistentLease>> = serde_yaml::from_slice(&buf)?;
        let now = Utc::now();
        for (service_name, persistent_leases) in persistent_leases.iter() {
            if let Some(broker_rw) = self.services.get(service_name) {
                let mut broker = broker_rw.write().await;
                let mut cred_hashes = broker.service.available_creds.drain(..)
                    .map(|cred| (cred.cred_hash(), cred))
                    .collect::<HashMap<_, _>>();

                for persistent_lease in persistent_leases.iter() {
                    if persistent_lease.expiration.is_expired(&now) {
                        continue;
                    }
                    if let Some(cred) = cred_hashes.remove(&persistent_lease.cred_hash) {
                        broker.service.leases.insert(
                            persistent_lease.lease_id.clone(),
                            Lease {
                                id: persistent_lease.lease_id.clone(),
                                user: cred.user,
                                password: cred.password,
                                client_name: persistent_lease.client_name.clone(),
                                expiration: persistent_lease.expiration.clone(),
                            },
                        );
                    }
                }
                for (_, cred) in cred_hashes.drain() {
                    broker.service.available_creds.push_back(cred);
                }
            }
        }

        Ok(())
    }
}


#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();
    println!("{} (v{})", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));

    let main_args: MainArgs = argh::from_env();
    let cfg = config::read_config(main_args.config_file)?;
    let app_state = {
        let mut a_state = AppState::from_cfg(&cfg);
        if let Some(persistent_leases_filename) = &cfg.persistent_leases_filename {
            match a_state.load_persistent_leases(Path::new(&persistent_leases_filename)).await {
                Ok(_) => info!("Persistent leases loaded"),
                Err(e) => warn!("Could not load persistent leases: {}", e.to_string())
            }
        }

        web::Data::new(a_state)
    };

    let app_state_for_save = app_state.clone();

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
    } else {
        server.bind(cfg.listen_on)?
    }
        .keep_alive(0)
        .run()
        .await
        .wrap_err_with(|| "failed to run webserver")?;

    // save leases on exit
    if let Some(persistent_leases_filename) = cfg.persistent_leases_filename {
        app_state_for_save.persist_leases(Path::new(&persistent_leases_filename)).await?
    }

    Ok(())
}

#[derive(Serialize)]
struct ServiceOverview {
    pub credentials_in_use: usize,
    pub credentials_available: usize,
    pub clients_waiting: usize,
}

#[derive(Serialize)]
struct Overview {
    pub services: HashMap<String, ServiceOverview>
}

async fn overview(app_state: web::Data<AppState>) -> actix_web::Result<HttpResponse> {
    let mut overview = Overview { services: Default::default() };
    for (service_name, broker_rw) in app_state.services.iter() {
        let broker = broker_rw.read().await;
        let s_overview = ServiceOverview {
            credentials_available: broker.service.leases_available(),
            credentials_in_use: broker.service.leases_in_use(),
            clients_waiting: broker.waiters.len(),
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

impl From<Lease> for GetLeaseResponse {
    fn from(lease: Lease) -> Self {
        Self {
            lease: lease.id.0,
            user: lease.user,
            password: lease.password,
            expires_on: lease.expiration.expires_on.to_rfc3339(),
        }
    }
}


async fn get_lease(request: HttpRequest, app_state: web::Data<AppState>, get_lease: web::Json<GetLeaseRequest>) -> impl Responder {
    let useragent = if let Some(ua_header) = request.headers().get("User-Agent") {
        match ua_header.to_str() {
            Ok(v) => Some(v.to_string()),
            Err(_) => None
        }
    } else {
        None
    }.unwrap_or_else(|| "<empty user-agent>".to_string());

    let service_name = ServiceName::from(get_lease.service.clone());

    if let Some(broker_rw) = app_state.services.get(&service_name) {
        let mut broker = broker_rw.write().await;
        let receiver = broker.obtain_lease(useragent.clone()).await;
        HttpResponse::Ok()
            .content_type("application/json")
            .streaming(receiver)
    } else {
        warn!("credential request for unknown service '{}' from client {} received",
              get_lease.service, useragent);
        HttpResponse::NotFound()
            .json(ErrorResponse {
                message: format!("unknown service \"{}\"", get_lease.service)
            })
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
    for (service_name, broker_rw) in app_state.services.iter() {
        let mut broker = broker_rw.write().await;
        if let Some(release) = broker.release(lease_id.clone()).await {
            info!("credential for service {} was in use for {:.3} secs by client '{}'",
                  service_name.0, release.duration.num_milliseconds() as f64 / 1000.0, release.client_name);
            break;
        }
    }
    Ok(HttpResponse::Ok().json(ClearLeaseResponse {}))
}
