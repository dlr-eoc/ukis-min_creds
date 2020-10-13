#[macro_use]
extern crate log;

use std::collections::{HashMap, HashSet, VecDeque};
use std::iter::FromIterator;
use std::ops::Add;
use std::sync::{Arc, Mutex};

use actix::{Actor, AsyncContext, Context};
use actix::clock::Duration;
use actix_web::{App, HttpResponse, HttpServer, middleware, web};
use actix_web::rt::Arbiter;
use actix_web::rt::time::delay_for;
use actix_web_httpauth::extractors::AuthenticationError;
use actix_web_httpauth::extractors::bearer::Config;
use actix_web_httpauth::middleware::HttpAuthentication;
use argh::FromArgs;
use chrono::prelude::*;
use eyre::{Result, WrapErr};
use serde::{Deserialize, Serialize};
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

#[derive(Clone)]
struct Lease {
    pub id: LeaseId,
    pub user: String,
    pub password: String,
    pub expires_on: DateTime<Utc>,
}

impl Lease {
    fn from_cred(cred: Cred, expiration_duration: chrono::Duration) -> Lease {
        Lease {
            id: LeaseId(Uuid::new_v4().to_string()),
            user: cred.user,
            password: cred.password,
            expires_on: Utc::now().add(expiration_duration),
        }
    }
}

#[derive(Eq, PartialEq, Hash, Debug)]
struct ServiceName(String);

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

    fn get_lease(&mut self) -> Option<Lease> {
        self.clear_expired_leases();
        if let Some(cred) = self.available_creds.pop_front() {
            let lease = Lease::from_cred(cred, self.expires_in);
            self.leases.insert(lease.id.clone(), lease.clone());
            Some(lease)
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
    pub services: Arc<Mutex<HashMap<ServiceName, Service>>>
}

impl Actor for Cleaner {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.run_interval(Duration::new(3, 0), |this, _ctx| {
            Arbiter::spawn(Cleaner::clean(this.services.clone()));
        });
    }
}

impl Cleaner {
    async fn clean(services: Arc<Mutex<HashMap<ServiceName, Service>>>) {
        let mut locked = services.lock().unwrap();
        for (service_name, service) in locked.iter_mut() {
            let n_expired = service.clear_expired_leases();
            if n_expired > 0 {
                info!("Cleared {} expired leases for service {}", n_expired, service_name.0)
            }
        }
    }
}

struct AppState {
    pub services: Arc<Mutex<HashMap<ServiceName, Service>>>,
    pub access_tokens: HashSet<String>,
    pub web_path: String,
}

impl AppState {
    fn from_cfg(cfg: &config::Config) -> Self {
        Self {

            services: Arc::new(Mutex::new(
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
                    (ServiceName(s_name.clone()), s)
                }).collect::<HashMap<_, _>>()
            )),

            access_tokens: HashSet::from_iter(cfg.access_tokens.iter().cloned()),
            web_path: cfg.web_path.clone(),
        }
    }
}


#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();

    let main_args: MainArgs = argh::from_env();
    let cfg = config::read_config(main_args.config_file)?;
    let appstate = web::Data::new(AppState::from_cfg(&cfg));

    Cleaner { services: appstate.services.clone() }.start();

    let web_path = cfg.web_path.clone();
    info!("Starting webserver on {} using path {}", cfg.listen_on, cfg.web_path);
    HttpServer::new(move || {
        let auth = HttpAuthentication::bearer(|req, creds| async move {
            let config = req
                .app_data::<Config>()
                .map(|data| data.clone())
                .unwrap_or_else(Default::default);

            let appstate = req.app_data::<web::Data<AppState>>()
                .expect("could not access appstate");
            let token = creds.token().to_string();
            if appstate.access_tokens.contains(&token) {
                Ok(req)
            } else {
                Err(AuthenticationError::from(config).into())
            }
        });
        App::new()
            .app_data(appstate.clone())
            .wrap(middleware::Logger::default())
            .service(
                web::scope(&web_path)
                    .wrap(auth)
                    .route("", web::get().to(overview))
                    .route("/get", web::post().to(get_lease))
                    .route("/release", web::post().to(clear_lease))
            )
    })
        .bind(cfg.listen_on)?
        .run()
        .await
        .wrap_err_with(|| "failed to run webserver")
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

async fn overview(appstate: web::Data<AppState>) -> actix_web::Result<HttpResponse> {
    let locked = appstate.services.lock().unwrap();
    Ok(HttpResponse::Ok().json(
        Overview {
            services: locked.iter().map(|(s_name, s)| {
                let s_overview = ServiceOverview {
                    leases_available: s.leases_available(),
                    leases_in_use: s.leases_in_use(),
                };
                (s_name.0.clone(), s_overview)
            }).collect()
        }
    ))
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

async fn get_lease(appstate: web::Data<AppState>, get_lease: web::Json<GetLeaseRequest>) -> actix_web::Result<HttpResponse> {
    loop {
        {
            let mut locked = appstate.services.lock().unwrap();
            if let Some(service) = locked.get_mut(&ServiceName(get_lease.service.clone())) {
                if let Some(lease) = service.get_lease() {
                    return Ok(HttpResponse::Ok().json(GetLeaseResponse {
                        lease: lease.id.0,
                        user: lease.user,
                        password: lease.password,
                        expires_on: lease.expires_on.to_rfc3339(),
                    }));
                }
            } else {
                return Ok(
                    HttpResponse::NotFound()
                        .json(ErrorResponse {
                            message: format!("unknown service \"{}\"", get_lease.service)
                        })
                );
            }
        } // releases the lock
        delay_for(Duration::from_millis(300)).await
    }
}

#[derive(Deserialize)]
struct ClearLeaseRequest {
    pub lease: String
}

#[derive(Serialize)]
struct ClearLeaseResponse {}

async fn clear_lease(appstate: web::Data<AppState>, clear_lease_req: web::Json<ClearLeaseRequest>) -> actix_web::Result<HttpResponse> {
    let mut locked = appstate.services.lock().unwrap();
    for (_, service) in locked.iter_mut() {
        service.leases.remove(&LeaseId(clear_lease_req.lease.clone()));
    }
    Ok(HttpResponse::Ok().json(ClearLeaseResponse {}))
}
