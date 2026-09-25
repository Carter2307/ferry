//! Naming conventions for Docker resources. Every crate that creates or looks
//! up Docker objects MUST go through [`Naming`] so they agree.

use std::collections::BTreeMap;

use crate::ids;

/// Label present on every Ferry-managed container/volume (`"true"`).
pub const LABEL_MANAGED: &str = "ferry.managed";
/// Label holding the owning server's `name_prefix`.
pub const LABEL_INSTANCE: &str = "ferry.instance";
/// Label: `service` | `job` | `datastore`.
pub const LABEL_ROLE: &str = "ferry.role";
/// Label: owning service id.
pub const LABEL_SERVICE: &str = "ferry.service";
/// Label: deploy id a service container belongs to.
pub const LABEL_DEPLOY: &str = "ferry.deploy";
/// Label: job run id.
pub const LABEL_JOB: &str = "ferry.job";
/// Label: datastore id.
pub const LABEL_DATASTORE: &str = "ferry.datastore";

pub const ROLE_SERVICE: &str = "service";
pub const ROLE_JOB: &str = "job";
pub const ROLE_DATASTORE: &str = "datastore";

/// Derives Docker names from the configured prefix (default `ferry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Naming {
    prefix: String,
}

impl Naming {
    pub fn new(prefix: &str) -> Self {
        Naming { prefix: prefix.to_string() }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The user-defined bridge network all workloads join (private networking).
    pub fn network(&self) -> String {
        self.prefix.clone()
    }

    /// Repository part of built images: `ferry/web`.
    pub fn image_repo(&self, service_name: &str) -> String {
        format!("{}/{}", self.prefix, service_name)
    }

    /// Tag for an image built by a deploy: `ferry/web:dep-…`.
    pub fn image_tag(&self, service_name: &str, deploy_id: &str) -> String {
        format!("{}:{}", self.image_repo(service_name), deploy_id)
    }

    /// Container name for one instance of a deploy: `ferry-web-1a2b3c4d-9f8e7d`.
    /// `unique` should be random-ish (e.g. `ids::random_secret(6)`).
    pub fn service_container(&self, service_name: &str, deploy_id: &str, unique: &str) -> String {
        format!("{}-{}-{}-{}", self.prefix, service_name, ids::short(deploy_id), unique)
    }

    /// Container name for a job run: `ferry-job-web-1a2b3c4d`.
    pub fn job_container(&self, service_name: &str, job_id: &str) -> String {
        format!("{}-job-{}-{}", self.prefix, service_name, ids::short(job_id))
    }

    /// Container name for a datastore: `ferry-ds-db`.
    pub fn datastore_container(&self, datastore_name: &str) -> String {
        format!("{}-ds-{}", self.prefix, datastore_name)
    }

    /// Data volume of a datastore: `ferry-ds-db-data`.
    pub fn datastore_volume(&self, datastore_name: &str) -> String {
        format!("{}-ds-{}-data", self.prefix, datastore_name)
    }

    /// Persistent disk volume of a service: `ferry-svc-srv-…-disk`.
    pub fn service_volume(&self, service_id: &str) -> String {
        format!("{}-svc-{}-disk", self.prefix, service_id)
    }

    /// Labels common to every managed resource.
    pub fn base_labels(&self, role: &str) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(LABEL_MANAGED.to_string(), "true".to_string());
        m.insert(LABEL_INSTANCE.to_string(), self.prefix.clone());
        m.insert(LABEL_ROLE.to_string(), role.to_string());
        m
    }

    /// Labels for a service instance container.
    pub fn service_labels(&self, service_id: &str, deploy_id: &str) -> BTreeMap<String, String> {
        let mut m = self.base_labels(ROLE_SERVICE);
        m.insert(LABEL_SERVICE.to_string(), service_id.to_string());
        m.insert(LABEL_DEPLOY.to_string(), deploy_id.to_string());
        m
    }

    /// Labels for a job container.
    pub fn job_labels(&self, service_id: &str, job_id: &str) -> BTreeMap<String, String> {
        let mut m = self.base_labels(ROLE_JOB);
        m.insert(LABEL_SERVICE.to_string(), service_id.to_string());
        m.insert(LABEL_JOB.to_string(), job_id.to_string());
        m
    }

    /// Labels for a datastore container / volume.
    pub fn datastore_labels(&self, datastore_id: &str) -> BTreeMap<String, String> {
        let mut m = self.base_labels(ROLE_DATASTORE);
        m.insert(LABEL_DATASTORE.to_string(), datastore_id.to_string());
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let n = Naming::new("ferry");
        assert_eq!(n.image_tag("web", "dep-0123456789abcdef0123"), "ferry/web:dep-0123456789abcdef0123");
        assert_eq!(n.service_container("web", "dep-0123456789abcdef0123", "xyz"), "ferry-web-cdef0123-xyz");
        assert_eq!(n.datastore_volume("db"), "ferry-ds-db-data");
        assert_eq!(n.service_labels("srv-1", "dep-1")[LABEL_INSTANCE], "ferry");
    }
}
