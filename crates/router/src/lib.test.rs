//! Cross-cutting helper contracts: type-URL parsing and next-sequence
//! (transliterated from engine_boundaries_test.go).

use super::*;

#[test]
fn type_name_from_url_strips_prefix() {
    assert_eq!(
        type_name_from_url("type.googleapis.com/examples.CardsDealt"),
        "examples.CardsDealt"
    );
    assert_eq!(
        type_name_from_url("examples.CardsDealt"),
        "examples.CardsDealt"
    );
}

#[test]
fn next_sequence_absent_book_is_zero() {
    assert_eq!(next_sequence(None), 0);
    assert_eq!(
        next_sequence(Some(&pb::EventBook {
            next_sequence: 9,
            ..Default::default()
        })),
        9
    );
}

// Host bytes decode as the message asked for; bytes that do not are
// ANY_DECODE_FAILED naming it (decision 14.37).
#[test]
fn decode_message_decodes_or_names_the_message_type() {
    use prost::Message;
    let sent = pb::ContextualCommand {
        conflict_nonce: vec![7, 7].into(),
        ..Default::default()
    };
    assert_eq!(
        decode_message::<pb::ContextualCommand>(&sent.encode_to_vec()),
        Ok(sent)
    );
    let err =
        decode_message::<pb::ContextualCommand>(&[0xff, 0xff, 0xff]).expect_err("undecodable");
    assert_eq!(err.code, error::codes::ANY_DECODE_FAILED);
    assert_eq!(
        err.extras.get("message_type").map(String::as_str),
        Some("io.angzarr.v1.ContextualCommand")
    );
}

// Decision 14.37 (owner's rulings, 2026-10-29, late): prost's DecodeError
// is a third-party library's error, so its own text travels verbatim as
// `ext_err_txt` (prost gives it no numeric code).
#[test]
fn undecodable_bytes_carry_the_decoders_own_text() {
    let garbage = [0xff, 0xff, 0xff];
    let prost_text = <pb::ContextualCommand as prost::Message>::decode(&garbage[..])
        .expect_err("undecodable")
        .to_string();
    let err = decode_message::<pb::ContextualCommand>(&garbage).expect_err("undecodable");
    assert_eq!(
        err.extras.get(error::extras::EXT_ERR_TXT),
        Some(&prost_text),
        "ext_err_txt (all: {:?})",
        err.extras
    );
}
