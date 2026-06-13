use std::collections::HashMap;

use crate::job::Job;

#[cfg(test)]
use bgrun_proto::JobState;

/// In-memory registry of all jobs managed by the daemon.
#[derive(Debug)]
pub struct JobStore {
    jobs: HashMap<String, Job>,
    name_index: HashMap<String, String>,
}

impl JobStore {
    /// Creates an empty job store.
    pub fn new() -> Self {
        JobStore {
            jobs: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    /// Inserts a job into the store. If the job has a name, it is indexed.
    pub fn insert(&mut self, job: Job) {
        if let Some(ref name) = job.name {
            self.name_index.insert(name.clone(), job.id.clone());
        }
        self.jobs.insert(job.id.clone(), job);
    }

    /// Returns a reference to a job by ID.
    pub fn get(&self, id: &str) -> Option<&Job> {
        self.jobs.get(id)
    }

    /// Returns a mutable reference to a job by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Job> {
        self.jobs.get_mut(id)
    }

    /// Finds a job by its name (case-sensitive).
    pub fn find_by_name(&self, name: &str) -> Option<&Job> {
        self.name_index.get(name).and_then(|id| self.jobs.get(id))
    }

    /// Resolves a query to a canonical job UUID.
    ///
    /// Tries in order:
    /// 1. Exact UUID match
    /// 2. Exact name match
    /// 3. Unique prefix match (min 4 chars, at most one job starts with the prefix)
    pub fn resolve_id(&self, query: &str) -> Option<String> {
        // 1. Exact UUID match
        if self.jobs.contains_key(query) {
            return Some(query.to_string());
        }
        // 2. Exact name match
        if let Some(uuid) = self.name_index.get(query) {
            if self.jobs.contains_key(uuid) {
                return Some(uuid.clone());
            }
        }
        // 3. Unique prefix match (min 4 chars)
        if query.len() >= 4 {
            let matches: Vec<&String> = self
                .jobs
                .keys()
                .filter(|id| id.starts_with(query))
                .collect();
            if matches.len() == 1 {
                return Some(matches[0].clone());
            }
        }
        None
    }

    /// Lists all jobs, optionally filtered by workspace.
    /// When `workspace` is None, returns all jobs.
    pub fn list_workspace(&self, workspace: Option<&str>) -> Vec<&Job> {
        self.jobs
            .values()
            .filter(|j| workspace.is_none_or(|ws| j.workspace.as_deref() == Some(ws)))
            .collect()
    }

    /// Removes a job from the store by ID. Returns the removed job if it existed.
    pub fn remove(&mut self, id: &str) -> Option<Job> {
        if let Some(job) = self.jobs.remove(id) {
            if let Some(ref name) = job.name {
                self.name_index.remove(name);
            }
            Some(job)
        } else {
            None
        }
    }
}

impl Default for JobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job(id: &str, name: Option<&str>, workspace: Option<&str>) -> Job {
        Job::new(
            id.into(),
            vec!["cmd".into()],
            name.map(|s| s.into()),
            workspace.map(|s| s.into()),
        )
    }

    #[test]
    fn test_insert_and_get() {
        let mut store = JobStore::new();
        let job = make_job("id1", None, None);
        store.insert(job);

        let retrieved = store.get("id1").unwrap();
        assert_eq!(retrieved.id, "id1");
    }

    #[test]
    fn test_get_mut_allows_modification() {
        let mut store = JobStore::new();
        let mut job = make_job("id1", None, None);
        job.state = JobState::Running;
        store.insert(job);

        let stored = store.get_mut("id1").unwrap();
        stored.transition(JobState::Exited).unwrap();
        assert_eq!(store.get("id1").unwrap().state, JobState::Exited);
    }

    #[test]
    fn test_get_nonexistent() {
        let store = JobStore::new();
        assert!(store.get("missing").is_none());
    }

    #[test]
    fn test_find_by_name() {
        let mut store = JobStore::new();
        store.insert(make_job("id1", Some("server"), None));

        let found = store.find_by_name("server").unwrap();
        assert_eq!(found.id, "id1");
    }

    #[test]
    fn test_find_by_name_case_sensitive() {
        let mut store = JobStore::new();
        store.insert(make_job("id1", Some("Server"), None));

        assert!(store.find_by_name("server").is_none());
        assert!(store.find_by_name("Server").is_some());
    }

    #[test]
    fn test_find_by_name_missing() {
        let store = JobStore::new();
        assert!(store.find_by_name("nothing").is_none());
    }

    #[test]
    fn test_list_workspace_all() {
        let mut store = JobStore::new();
        store.insert(make_job("id1", None, Some("ws1")));
        store.insert(make_job("id2", None, Some("ws2")));
        store.insert(make_job("id3", None, None));

        let all = store.list_workspace(None);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_list_workspace_filtered() {
        let mut store = JobStore::new();
        store.insert(make_job("id1", None, Some("ws1")));
        store.insert(make_job("id2", None, Some("ws2")));
        store.insert(make_job("id3", None, Some("ws1")));

        let ws1 = store.list_workspace(Some("ws1"));
        assert_eq!(ws1.len(), 2);
        assert!(ws1.iter().all(|j| j.workspace.as_deref() == Some("ws1")));
    }

    #[test]
    fn test_remove() {
        let mut store = JobStore::new();
        store.insert(make_job("id1", Some("srv"), None));

        let removed = store.remove("id1").unwrap();
        assert_eq!(removed.id, "id1");
        assert!(store.get("id1").is_none());
        assert!(store.find_by_name("srv").is_none());
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut store = JobStore::new();
        assert!(store.remove("missing").is_none());
    }

    #[test]
    fn test_default_is_empty() {
        let store = JobStore::default();
        assert_eq!(store.list_workspace(None).len(), 0);
    }
}
