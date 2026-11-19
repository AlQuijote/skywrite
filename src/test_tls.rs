//! TEST ONLY: owned-loopback TLS chains for the pinned-download and public-access
//! tests. A fresh CA and leaves are generated once per test process and never
//! leave it. Platform verifiers enforce server-certificate rules (macOS rejects
//! lifetimes over 398 days, missing SANs or a missing serverAuth EKU), so leaves
//! are short-lived and carry explicit SAN, EKU, key usage and basic constraints.
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use std::path::PathBuf;
use std::sync::OnceLock;

pub(crate) struct Leaf {
    cert_pem: String,
    key_pem: String,
}

pub(crate) struct Chain {
    ca_pem: String,
    /// SANs: pinned.invalid, localhost, 127.0.0.1, 127.0.0.2 and ::1.
    pub(crate) server: Leaf,
    /// SAN: mismatch.invalid only, so IP and other names fail verification.
    pub(crate) mismatch: Leaf,
}

/// A leaf's PEM files in a private directory removed when this is dropped.
pub(crate) struct LeafFiles {
    _dir: tempfile::TempDir,
    pub(crate) cert: PathBuf,
    pub(crate) key: PathBuf,
}

impl Leaf {
    pub(crate) fn files(&self) -> LeafFiles {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("server.pem");
        let key = dir.path().join("server.key");
        std::fs::write(&cert, &self.cert_pem).unwrap();
        std::fs::write(&key, &self.key_pem).unwrap();
        LeafFiles {
            _dir: dir,
            cert,
            key,
        }
    }
}

impl Chain {
    pub(crate) fn ca(&self) -> reqwest::Certificate {
        reqwest::Certificate::from_pem(self.ca_pem.as_bytes()).unwrap()
    }
}

pub(crate) fn chain() -> &'static Chain {
    static CHAIN: OnceLock<Chain> = OnceLock::new();
    CHAIN.get_or_init(generate)
}

fn validity(params: &mut CertificateParams) {
    // One day of backdating tolerates coarse clocks; a week covers any test run.
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(7);
}

fn generate() -> Chain {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::default();
    ca.distinguished_name
        .push(DnType::CommonName, "Skywrite TEST ONLY CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    validity(&mut ca);
    let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
    let issuer = Issuer::new(ca, ca_key);
    let leaf = |names: &[&str]| {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        params
            .distinguished_name
            .push(DnType::CommonName, "Skywrite TEST ONLY server");
        params.subject_alt_names = names
            .iter()
            .map(|name| match name.parse() {
                Ok(ip) => SanType::IpAddress(ip),
                Err(_) => SanType::DnsName((*name).try_into().unwrap()),
            })
            .collect();
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        validity(&mut params);
        Leaf {
            cert_pem: params.signed_by(&key, &issuer).unwrap().pem(),
            key_pem: key.serialize_pem(),
        }
    };
    Chain {
        ca_pem,
        server: leaf(&[
            "pinned.invalid",
            "localhost",
            "127.0.0.1",
            "127.0.0.2",
            "::1",
        ]),
        mismatch: leaf(&["mismatch.invalid"]),
    }
}
