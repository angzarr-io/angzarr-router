//! Destinations: the declared output domains, in declaration order.

use crate::destinations::Destinations;

#[test]
fn has_reflects_declared_domains() {
    let d = Destinations::new(["inventory", "fulfillment"]);
    assert!(d.has("inventory"));
    assert!(d.has("fulfillment"));
    assert!(!d.has("shipping"));
}

#[test]
fn domains_keep_declaration_order() {
    let d = Destinations::new(["inventory", "fulfillment"]);
    assert_eq!(
        d.domains(),
        ["inventory".to_string(), "fulfillment".to_string()]
    );
}

#[test]
fn no_declared_domains_has_none() {
    let d = Destinations::new(Vec::<String>::new());
    assert!(!d.has("inventory"));
    assert!(d.domains().is_empty());
}
