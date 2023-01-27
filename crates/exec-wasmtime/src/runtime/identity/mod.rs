// SPDX-License-Identifier: Apache-2.0

//! Functionality for establishing keep identity.

mod pki;
mod platform;

use super::super::{Package, PackageSpec};

use pki::PrivateKeyInfoExt;
use platform::{Platform, Technology};

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{bail, Context};
use const_oid::db::rfc4519::COMMON_NAME;
use const_oid::db::rfc5280::{
    ID_CE_BASIC_CONSTRAINTS, ID_CE_EXT_KEY_USAGE, ID_CE_KEY_USAGE, ID_KP_CLIENT_AUTH,
    ID_KP_SERVER_AUTH,
};
use const_oid::db::rfc5912::{SECP_256_R_1, SECP_384_R_1};
use const_oid::AssociatedOid;
use drawbridge_client::types::digest::{Algorithm, ContentDigest};
use getrandom::getrandom;
use pkcs8::PrivateKeyInfo;
use serde::Serialize;
use sha2::{Digest, Sha256, Sha384};
use url::Url;
use wiggle::tracing::instrument;
use x509_cert::attr::{Attribute, AttributeTypeAndValue};
use x509_cert::der::asn1::{BitStringRef, UIntRef};
use x509_cert::der::{AnyRef, Decode, Encode};
use x509_cert::ext::pkix::{BasicConstraints, ExtendedKeyUsage, KeyUsage, KeyUsages};
use x509_cert::ext::Extension;
use x509_cert::name::{RdnSequence, RelativeDistinguishedName};
use x509_cert::request::{CertReq, CertReqInfo, ExtensionReq};
use x509_cert::time::Validity;
use x509_cert::{Certificate, PkiPath, TbsCertificate};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum Peer {
    #[serde(rename = "anonymous")]
    Anonymous,
    #[serde(rename = "local")]
    Local { digest: String },
    #[serde(rename = "keep")]
    Keep { workload: String, digest: String },
}

impl Peer {
    pub fn from_certs(certs: impl IntoIterator<Item = impl AsRef<[u8]>>) -> anyhow::Result<Self> {
        let names = certs.into_iter().next().map(|cert| {
            let cert = cert.as_ref();
            let Certificate {
                tbs_certificate, ..
            } = Certificate::from_der(cert).expect("failed to parse certificate");
            // TODO: check signature
            tbs_certificate
                .subject
                .0
                .into_iter()
                .flat_map(|RelativeDistinguishedName(rdn)| {
                    rdn.into_vec()
                        .into_iter()
                        .filter_map(|AttributeTypeAndValue { oid, value }| {
                            matches!(oid, COMMON_NAME).then(|| {
                                value
                                    .utf8_string()
                                    .expect("common name is not a UTF-8 string")
                                    .to_string()
                            })
                        })
                })
                .collect::<HashSet<_>>()
        });
        let mut names = if let Some(names) = names {
            names.into_iter()
        } else {
            return Ok(Self::Anonymous);
        };
        let slug = if let Some(slug) = names.next() {
            slug
        } else {
            return Ok(Self::Anonymous);
        };
        let (workload, digest) = slug.rsplit_once('@').unwrap();
        let digest = digest.replace('-', "+").replace('_', "/");

        // TODO: This should be based on signatures in the certificate chain
        if workload == "localhost" {
            Ok(Self::Local { digest })
        } else {
            Ok(Self::Keep {
                workload: workload.into(),
                digest,
            })
        }
    }
}

fn csr(
    pki: &PrivateKeyInfo<'_>,
    exts: Vec<Extension<'_>>,
    pkg: &Package,
    wdig: &ContentDigest,
) -> anyhow::Result<Vec<u8>> {
    // Request the extensions.
    let req = ExtensionReq::from(exts)
        .to_vec()
        .context("failed to encode an extension request")?;

    // Embed the extension requests in an attribute.
    let any = AnyRef::from_der(&req).context("failed to parse extension request from DER")?;
    let att_values = vec![any]
        .try_into()
        .context("failed to generate attribute values")?;
    let att = Attribute {
        oid: ExtensionReq::OID,
        values: att_values,
    };

    let attributes = vec![att]
        .try_into()
        .context("failed to generate CRI attributes")?;
    let public_key = pki.public_key().context("failed to extract public key")?;

    let digest = wdig
        .get(&Algorithm::Sha256)
        .unwrap()
        .to_string()
        .trim_end_matches('=')
        .to_string()
        .replace('+', "-")
        .replace('/', "_");

    let subject = match pkg {
        Package::Remote(PackageSpec::Slug(ref slug)) => format!("CN={slug}@{digest}"),
        Package::Remote(PackageSpec::Url(ref url)) => format!("CN={url}@{digest}"),
        Package::Local { .. } => format!("CN=localhost@{digest}"),
    };
    let subject = RdnSequence::encode_from_string(&subject)
        .context("failed to encode RDN sequence to DER")?;
    let subject =
        RdnSequence::from_der(&subject).context("failed to parse RDN sequence from DER")?;

    // Create a certification request information structure.
    let info = CertReqInfo {
        version: x509_cert::request::Version::V1,
        attributes,
        subject,
        public_key,
    };

    let algorithm = pki
        .signs_with()
        .context("failed to query signing algorithm")?;
    let info_bytes = info.to_vec().context("failed to encode CRI")?;

    // Sign the request.
    let sig = pki
        .sign(&info_bytes, algorithm)
        .context("failed to sign request")?;
    let signature = BitStringRef::from_bytes(sig.as_ref()).context("failed to parse signature")?;
    CertReq {
        info,
        algorithm,
        signature,
    }
    .to_vec()
    .context("failed to encode CSR")
}

/// Generates a new private key and corresponding CSR
#[instrument]
pub fn generate(
    pkg: &Package,
    wdig: &ContentDigest,
) -> anyhow::Result<(Zeroizing<Vec<u8>>, Vec<u8>)> {
    let platform = Platform::get().context("failed to query platform")?;
    let cert_algo = match platform.technology() {
        Technology::Snp => SECP_384_R_1,
        Technology::Sgx => SECP_256_R_1,
        Technology::Kvm => SECP_256_R_1,
    };

    // Generate a keypair.
    let raw = PrivateKeyInfo::generate(cert_algo).context("failed to generate a private key")?;
    let pki = PrivateKeyInfo::from_der(raw.as_ref())
        .context("failed to parse DER-encoded private key")?;
    let der = pki.public_key().unwrap().to_vec().unwrap();

    let mut key_hash = [0u8; 64];
    match platform.technology() {
        Technology::Snp => {
            let hash = Sha384::digest(der);
            key_hash[..48].copy_from_slice(&hash);
        }
        _ => {
            let hash = Sha256::digest(der);
            key_hash[..32].copy_from_slice(&hash);
        }
    };

    let attestation_report = platform.attest(&key_hash).context("failed to attest")?;

    // Create extensions.
    let ext = vec![Extension {
        extn_id: platform.technology().into(),
        critical: false,
        extn_value: &attestation_report,
    }];

    // Make a certificate signing request.
    let req = csr(&pki, ext, pkg, wdig).context("failed to generate a CSR")?;

    Ok((raw, req))
}

#[instrument(skip(csr))]
pub fn steward(url: &Url, csr: impl AsRef<[u8]>) -> anyhow::Result<Vec<Vec<u8>>> {
    if url.scheme() != "https" {
        bail!("refusing to use an unencrypted steward url");
    }

    // Send the attestation to the steward.
    let response = ureq::post(url.as_str())
        .set("Content-Type", "application/pkcs10")
        .send_bytes(csr.as_ref())?;

    // Read the result.
    let mut body = Vec::new();
    response.into_reader().read_to_end(&mut body)?;

    // Decode the certificate chain.
    let path = PkiPath::from_der(&body)?;
    path.iter().rev().map(|c| Ok(c.to_vec()?)).collect()
}

#[instrument(skip(key))]
pub fn selfsigned(
    key: impl AsRef<[u8]>,
    pkg: &Package,
    wdig: &ContentDigest,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let pki = PrivateKeyInfo::from_der(key.as_ref())?;

    let digest = wdig
        .get(&Algorithm::Sha256)
        .unwrap()
        .to_string()
        .trim_end_matches('=')
        .to_string()
        .replace('+', "-")
        .replace('/', "_");

    // Create a relative distinguished name.
    let rdns = match pkg {
        Package::Remote(PackageSpec::Slug(ref slug)) => format!("CN={slug}@{digest}"),
        Package::Remote(PackageSpec::Url(ref url)) => format!("CN={url}@{digest}"),
        Package::Local { .. } => format!("CN=localhost@{digest}"),
    };
    let rdns = RdnSequence::encode_from_string(&rdns)?;

    // Create the extensions.
    let ku = KeyUsage(KeyUsages::DigitalSignature | KeyUsages::KeyEncipherment).to_vec()?;
    let eu = ExtendedKeyUsage(vec![ID_KP_SERVER_AUTH, ID_KP_CLIENT_AUTH]).to_vec()?;
    let bc = BasicConstraints {
        ca: false,
        path_len_constraint: None,
    }
    .to_vec()?;

    // Steward uses UUIDs as serial numbers, use 16-octet long serial number to loosely
    // resemble format used by the Steward.
    let mut serial = [0u8; 16];
    getrandom(&mut serial)?;

    // Create the certificate body.
    let tbs = TbsCertificate {
        version: x509_cert::Version::V3,
        serial_number: UIntRef::new(&serial)?,
        signature: pki.signs_with()?,
        issuer: RdnSequence::from_der(&rdns)?,
        validity: Validity::from_now(Duration::from_secs(60 * 60 * 24 * 365))?,
        subject: RdnSequence::from_der(&rdns)?,
        subject_public_key_info: pki.public_key()?,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: Some(vec![
            x509_cert::ext::Extension {
                extn_id: ID_CE_KEY_USAGE,
                critical: true,
                extn_value: &ku,
            },
            x509_cert::ext::Extension {
                extn_id: ID_CE_BASIC_CONSTRAINTS,
                critical: true,
                extn_value: &bc,
            },
            x509_cert::ext::Extension {
                extn_id: ID_CE_EXT_KEY_USAGE,
                critical: false,
                extn_value: &eu,
            },
        ]),
    };

    // Self-sign the certificate.
    let alg = tbs.signature;
    let sig = pki.sign(&tbs.to_vec()?, alg)?;
    let crt = Certificate {
        tbs_certificate: tbs,
        signature_algorithm: alg,
        signature: BitStringRef::from_bytes(&sig)?,
    }
    .to_vec()?;
    Ok(vec![crt])
}
