use crate::srp::{self, SrpParams};
use crate::world::World;
use std::sync::Arc;

fn params(w: &World) -> Result<Arc<SrpParams>, String> {
    w.srp
        .clone()
        .ok_or_else(|| "SRP is not configured: declare a resource in resources.srp".to_string())
}

/// The salt is taken from the argument if the step received one, otherwise generated.
pub fn generate_verifier(
    w: &mut World,
    identity: &str,
    password: &str,
    salt: Option<&str>,
    prefix: &str,
) -> Result<(), String> {
    let p = params(w)?;
    let salt = salt.map(str::to_string).unwrap_or_else(srp::generate_salt);
    let verifier = srp::compute_verifier(&p, &salt, identity, password)?;
    w.vars.set(&format!("{prefix}_salt"), salt);
    w.vars.set(&format!("{prefix}_verifier"), verifier);
    Ok(())
}

/// The private `a` is stored in a regular variable: the completion step reads
/// it from there, no separate state is needed for this.
pub fn start_login(w: &mut World, prefix: &str) -> Result<(), String> {
    let p = params(w)?;
    let (a, a_pub) = srp::start_login(&p);
    w.vars.set(&format!("{prefix}_a"), a);
    w.vars.set(&format!("{prefix}_A"), a_pub);
    Ok(())
}

pub fn complete_login(
    w: &mut World,
    prefix: &str,
    identity: &str,
    password: &str,
    salt: &str,
    b: &str,
) -> Result<(), String> {
    let p = params(w)?;
    let a = w
        .vars
        .get(&format!("{prefix}_a"))
        .ok_or_else(|| {
            format!(
                "run the step \"I start an SRP login as {prefix:?}\" before completing the login"
            )
        })?
        .to_string();
    let proof = srp::complete_login(&p, salt, identity, password, &a, b)?;
    w.vars.set(&format!("{prefix}_M1"), proof.m1);
    w.vars.set(&format!("{prefix}_M2"), proof.m2);
    w.vars
        .set(&format!("{prefix}_sessionKey"), proof.session_key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srp::{HashAlg, RFC5054_4096_PRIME_HEX, SrpParams, Variant};
    use std::sync::Arc;

    fn world() -> World {
        let mut by_name = std::collections::HashMap::new();
        by_name.insert(
            "main".to_string(),
            crate::http::ApiResource::new(
                "http://x.local",
                5,
                Vec::new(),
                crate::options::Options::default(),
            )
            .expect("valid base_url"),
        );
        let apis = Arc::new(
            crate::http::Apis::new(by_name, Some("main".to_string())).expect("default declared"),
        );
        World::new(
            apis,
            Arc::new(crate::unique::Generator::new()),
            crate::db::DbHandle::new(None, String::new()),
            Some(Arc::new(SrpParams {
                variant: Variant::HexString,
                prime: num_bigint::BigUint::parse_bytes(RFC5054_4096_PRIME_HEX.as_bytes(), 16)
                    .expect("valid prime"),
                generator: num_bigint::BigUint::from(5u32),
                hash: HashAlg::Sha256,
            })),
            None,
            crate::options::Options::default(),
        )
    }

    #[test]
    fn generating_a_verifier_exports_salt_and_verifier() {
        let mut w = world();
        generate_verifier(&mut w, "u@example.test", "secret", None, "reg").expect("step succeeds");
        let salt = w.vars.get("reg_salt").expect("salt exported").to_string();
        assert_eq!(salt.len(), 64);
        assert!(w.vars.get("reg_verifier").is_some(), "verifier exported");
    }

    #[test]
    fn a_supplied_salt_is_echoed_back_unchanged() {
        let mut w = world();
        generate_verifier(&mut w, "u@example.test", "secret", Some("0abc"), "reg")
            .expect("step succeeds");
        assert_eq!(w.vars.get("reg_salt"), Some("0abc"));
    }

    #[test]
    fn the_same_salt_and_password_give_the_same_verifier() {
        let mut w = world();
        generate_verifier(&mut w, "u@example.test", "secret", Some("0abc"), "one").expect("first");
        generate_verifier(&mut w, "u@example.test", "secret", Some("0abc"), "two").expect("second");
        assert_eq!(w.vars.get("one_verifier"), w.vars.get("two_verifier"));
    }

    #[test]
    fn starting_a_login_exports_both_ephemerals() {
        let mut w = world();
        start_login(&mut w, "srp").expect("step succeeds");
        assert!(w.vars.get("srp_A").is_some(), "public value exported");
        assert!(w.vars.get("srp_a").is_some(), "private value exported");
    }

    #[test]
    fn completing_a_login_exports_the_proofs() {
        let mut w = world();
        generate_verifier(&mut w, "u@example.test", "secret", None, "reg").expect("verifier");
        let salt = w.vars.get("reg_salt").expect("salt").to_string();
        let verifier = w.vars.get("reg_verifier").expect("verifier").to_string();
        start_login(&mut w, "srp").expect("start");

        let params = w.srp.clone().expect("params");
        let v = num_bigint::BigUint::parse_bytes(verifier.as_bytes(), 16).expect("valid hex");
        let b_priv = num_bigint::BigUint::from(0x5c2e91a0u32);
        let b_pub =
            (params.k() * v + params.generator.modpow(&b_priv, &params.prime)) % &params.prime;

        complete_login(
            &mut w,
            "srp",
            "u@example.test",
            "secret",
            &salt,
            &crate::srp::hex(&b_pub),
        )
        .expect("step succeeds");

        assert_eq!(w.vars.get("srp_M1").expect("M1").len(), 64);
        assert_eq!(w.vars.get("srp_M2").expect("M2").len(), 64);
        assert_eq!(w.vars.get("srp_sessionKey").expect("session key").len(), 64);
    }

    #[test]
    fn without_a_configured_resource_the_step_says_so() {
        let mut w = world();
        w.srp = None;
        let err = start_login(&mut w, "srp").unwrap_err();
        assert!(
            err.contains("resources.srp"),
            "error must point at config: {err}"
        );
    }
}
