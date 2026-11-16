//! `actor skin --expect-input-sha`: the stage refuses an input whose digest is not
//! the one the chain pinned, before it authors a single weight.
//!
//! The M2 audit was produced by a chain that consumed a superseded seat by name;
//! this contract is what makes that a refusal instead of a silent re-author.

use std::path::PathBuf;

use sculpto_engine::skin::{SkinParams, run};

fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sculpto-skin-sha-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn params(actor: PathBuf, expect: Option<&str>) -> SkinParams {
    SkinParams {
        actor,
        out: None,
        report: None,
        expect_input_sha: expect.map(str::to_owned),
    }
}

#[test]
fn a_wrong_input_sha_is_refused_before_the_actor_is_read() {
    let dir = work_dir("wrong");
    let actor = dir.join("actor.glb");
    std::fs::write(&actor, b"not a glb, only bytes to digest").expect("fixture");

    let error = run(&params(actor.clone(), Some("0000000000000000")))
        .expect_err("a digest that does not match must be refused");
    assert_eq!(error.code(), "skin_input_sha");
    assert!(
        error.message().contains("does not match"),
        "the message must name the mismatch: {}",
        error.message()
    );

    // An expectation shorter than eight hex digits is not evidence, so it is refused
    // on its own terms rather than matching everything.
    let error =
        run(&params(actor.clone(), Some("0000"))).expect_err("a four-digit expectation is refused");
    assert_eq!(error.code(), "skin_input_sha");
    assert!(
        error.message().contains("shorter than eight"),
        "the message must name the short expectation: {}",
        error.message()
    );

    // The matching prefix walks past the digest gate and fails on the file itself,
    // which proves the gate is not a blanket refusal.
    let digest = {
        use sha2::{Digest, Sha256};
        let bytes = std::fs::read(&actor).expect("fixture");
        format!("{:x}", Sha256::digest(&bytes))
    };
    let error = run(&params(actor, Some(&digest[..16]))).expect_err("the bytes are not an actor");
    assert_ne!(
        error.code(),
        "skin_input_sha",
        "the digest gate refused its own digest"
    );
    std::fs::remove_dir_all(&dir).ok();
}
