Feature: Order saga dispatch

  The OrderSaga proves the translation-side dispatch mechanisms the shared
  router must implement identically in every language: a declared source
  event emits a deferred command (angzarr_deferred provenance from the
  triggering event: its book's whole cover, its sequence and the command's
  emission index, never an explicit sequence; source_component is left for
  the coordinator) and its handler sees the triggering event's sequence, an
  undeclared event emits nothing, a source missing or empty is refused with a
  coded error, and a rejection notification in a saga's source emits nothing.
  Sagas never receive rejections, so a saga declaring a rejection handler is
  refused at registration.

  Deferred commands are byte-identical in every language: the parity saga
  turns one event into the same command twice, and each stamped command's
  deterministic encoding (proto canonical field order, no unknown fields)
  hashes to a locked SHA-256. Its source cover is domain "order", root bytes
  00..0f and correlation_id "corr-1"; its command's cover is domain
  "inventory", root bytes 10..1f and correlation_id "corr-1", with one page
  whose command is type URL "/example.Foo" carrying bytes 01020304.

  Scenario: a declared event emits a deferred command
    Given an order saga delivering to "inventory"
    When an Increased event at sequence 7 is dispatched
    Then the saga emits one command to "inventory"
    And the command is deferred from source sequence 7 at command index 0
    And the command leaves its source component to the coordinator

  Scenario: an undeclared event emits nothing
    Given an order saga delivering to "inventory"
    When a Reserve event is dispatched
    Then the saga emits no commands

  Scenario: a source with no pages is refused
    Given an order saga delivering to "inventory"
    When a source with no pages is dispatched
    Then the dispatch fails with EMPTY_SAGA_SOURCE

  Scenario: a request with no source is refused
    Given an order saga delivering to "inventory"
    When a request with no source is dispatched
    Then the dispatch fails with MISSING_SAGA_SOURCE

  Scenario: a rejection in the source is not a saga's to compensate
    Given an order saga delivering to "inventory"
    When a rejection of Reserve is dispatched
    Then the saga emits no commands
    And the saga injects no events

  Scenario: a saga handler sees the triggering event's sequence
    Given an order saga delivering to "inventory"
    When an Increased event at sequence 7 is dispatched
    Then the saga handler saw source sequence 7

  Scenario: a deferred command records the whole source cover
    Given an order saga delivering to "inventory"
    When an Increased event of order root "order-1" at sequence 7 is dispatched
    Then the command is deferred from order root "order-1"

  Scenario: deferred commands encode identically in every language
    Given a parity saga emitting the parity command twice
    When the parity source event at sequence 3 is dispatched
    Then the command at index 0 hashes to SHA-256 "10b1ce23a470f107662591a7da41830c724fdc0c9562824130f09b0a12f011f5"
    And the command at index 1 hashes to SHA-256 "a028d427e91c0e03b63b50dabe7c5184417ed7aaf558d62dfdb9baed1e3affd0"

  Scenario: a saga declaring a rejection handler is refused at registration
    When a saga declaring a compensation for Reserve is registered
    Then the registration is refused as INVALID_ARGUMENT
