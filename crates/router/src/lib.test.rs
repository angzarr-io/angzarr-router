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
