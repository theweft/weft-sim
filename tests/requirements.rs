// SPDX-FileCopyrightText: 2026 The weft authors
// SPDX-License-Identifier: MIT

//! Requirements gate: requirements/*.sdoc against the relation markers.
//!
//! weft-sim is a binary with unit tests in src/, a C module and a VM test script, so markers are
//! read from src/, module/ and guest/; a marker with `role=test` is a test reference, any other
//! an implementation reference.
//!
//! - Every marker names a known requirement.
//! - Every Active requirement verified by Test has an implementation and a test reference.
//! - Requirement ids are unique.
//!
//! `StrictDoc` renders the same links as HTML (`just requirements`); this test is the CI gate.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// Split so that this file does not contain a marker itself.
const MARKER: &str = concat!("@rel", "ation(");

struct Requirement {
    status: String,
    verification: String,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// (uid, requirement) in file order, duplicates included.
fn requirements() -> Vec<(String, Requirement)> {
    let mut out = Vec::new();
    for path in files(&root().join("requirements")) {
        if path.extension().is_none_or(|e| e != "sdoc") {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap();
        let mut current: Option<(String, Requirement)> = None;
        for line in src.lines() {
            if line == "[REQUIREMENT]" {
                out.extend(current.take());
                current = Some((
                    String::new(),
                    Requirement {
                        status: String::new(),
                        verification: String::new(),
                    },
                ));
            } else if let Some((uid, req)) = current.as_mut() {
                if let Some(v) = line.strip_prefix("UID: ") {
                    *uid = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("STATUS: ") {
                    req.status = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("VERIFICATION: ") {
                    req.verification = v.trim().to_string();
                }
            }
        }
        out.extend(current);
    }
    out
}

/// Requirement id -> locations of implementation markers and of test markers.
#[allow(clippy::type_complexity)]
fn references() -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let (mut implemented, mut tested): (BTreeMap<String, Vec<String>>, BTreeMap<_, Vec<_>>) =
        (BTreeMap::new(), BTreeMap::new());
    for dir in ["src", "module", "guest"] {
        for path in files(&root().join(dir)) {
            let Ok(src) = fs::read_to_string(&path) else {
                continue; // build outputs such as .ko files
            };
            let rel = path.strip_prefix(root()).unwrap().display().to_string();
            for (n, line) in src.lines().enumerate() {
                let Some(start) = line.find(MARKER) else {
                    continue;
                };
                let args = &line[start + MARKER.len()..];
                let args = &args[..args.find(')').unwrap_or(args.len())];
                let items: Vec<&str> = args.split(',').map(str::trim).collect();
                let target = if items.contains(&"role=test") {
                    &mut tested
                } else {
                    &mut implemented
                };
                for item in items {
                    if !item.is_empty() && !item.starts_with("scope=") && !item.starts_with("role=")
                    {
                        target
                            .entry(item.to_string())
                            .or_default()
                            .push(format!("{rel}:{}", n + 1));
                    }
                }
            }
        }
    }
    (implemented, tested)
}

#[test]
fn requirement_ids_are_unique_and_complete() {
    let reqs = requirements();
    assert!(!reqs.is_empty(), "no requirements found");
    let mut seen = BTreeSet::new();
    for (uid, req) in &reqs {
        assert!(!uid.is_empty(), "requirement without UID");
        assert!(seen.insert(uid.clone()), "duplicate requirement {uid}");
        assert!(!req.status.is_empty(), "{uid}: missing STATUS");
        assert!(!req.verification.is_empty(), "{uid}: missing VERIFICATION");
    }
}

#[test]
fn every_marker_names_a_known_requirement() {
    let known: BTreeSet<String> = requirements().into_iter().map(|(uid, _)| uid).collect();
    let (implemented, tested) = references();
    let unknown: Vec<String> = implemented
        .iter()
        .chain(tested.iter())
        .filter(|(id, _)| !known.contains(*id))
        .map(|(id, at)| format!("{id} at {}", at.join(", ")))
        .collect();
    assert!(unknown.is_empty(), "unknown requirements: {unknown:#?}");
}

#[test]
fn active_requirements_are_implemented_and_tested() {
    let (implemented, tested) = references();
    let mut missing = Vec::new();
    for (uid, req) in requirements() {
        if req.status != "Active" {
            continue;
        }
        if !implemented.contains_key(&uid) {
            missing.push(format!("{uid}: no implementation marker"));
        }
        if req.verification == "Test" && !tested.contains_key(&uid) {
            missing.push(format!("{uid}: no test marker (role=test)"));
        }
    }
    assert!(missing.is_empty(), "requirements gate: {missing:#?}");
}
