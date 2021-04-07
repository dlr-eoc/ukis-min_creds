use std::collections::{HashMap, VecDeque};
use std::ops::Add;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use uuid::Uuid;

pub struct Cred {
    pub user: String,
    pub password: String,
}

impl Cred {
    pub fn cred_hash(&self) -> String {
        Sha1::from(format!("{}|{}", self.user, self.password)).hexdigest()
    }
}

impl From<Lease> for Cred {
    fn from(lease: Lease) -> Self {
        Self {
            user: lease.user,
            password: lease.password,
        }
    }
}

#[derive(PartialEq, Eq, Hash, Debug, Clone, Serialize, Deserialize)]
pub struct LeaseId(pub String);

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

#[derive(Clone, Serialize, Deserialize)]
pub struct Expiration {
    pub expires_on: DateTime<Utc>,
    pub created_on: DateTime<Utc>,
}

impl Expiration {
    pub fn is_expired(&self, now: &DateTime<Utc>) -> bool {
        self.expires_on < *now
    }
}

#[derive(Clone)]
pub struct Lease {
    pub id: LeaseId,
    pub user: String,
    pub password: String,
    pub client_name: String,
    pub expiration: Expiration,
}

impl Lease {
    fn from_cred(cred: Cred, expiration_duration: chrono::Duration, client_name: &str) -> Lease {
        let now = Utc::now();
        Lease {
            id: LeaseId::new(),
            user: cred.user,
            password: cred.password,
            client_name: client_name.to_string(),
            expiration: Expiration {
                expires_on: now.add(expiration_duration),
                created_on: now,
            },
        }
    }
}

#[derive(Eq, PartialEq, Hash, Debug, Clone, Serialize, Deserialize)]
pub struct ServiceName(pub String);

impl From<String> for ServiceName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

pub struct LeaseRelease {
    pub duration: chrono::Duration,
    pub client_name: String,
}

pub struct Service {
    pub expires_in: chrono::Duration,
    pub leases: HashMap<LeaseId, Lease>,
    pub available_creds: VecDeque<Cred>,
}

impl Service {
    pub fn clear_expired_leases(&mut self) -> usize {
        let now = Utc::now();
        let mut n_expired = 0_usize;

        let mut leases_to_remove = vec![];
        for lease_id in self.leases.keys() {
            if self.leases[lease_id].expiration.is_expired(&now) {
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

    pub fn get_lease(&mut self, client_name: &str) -> Option<Lease> {
        self.clear_expired_leases();
        if let Some(cred) = self.available_creds.pop_front() {
            let lease = Lease::from_cred(cred, self.expires_in, client_name);
            self.leases.insert(lease.id.clone(), lease.clone());
            Some(lease)
        } else {
            None
        }
    }

    pub fn release(&mut self, lease_id: &LeaseId) -> Option<LeaseRelease> {
        if let Some(lease) = self.leases.remove(lease_id) {
            let lr = LeaseRelease {
                duration: Utc::now() - lease.expiration.created_on,
                client_name: lease.client_name.clone(),
            };
            self.available_creds.push_back(lease.into());
            Some(lr)
        } else {
            None
        }
    }

    pub fn leases_available(&self) -> usize {
        self.available_creds.len()
    }

    pub fn leases_in_use(&self) -> usize {
        self.leases.len()
    }
}
