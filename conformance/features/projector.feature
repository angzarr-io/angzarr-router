Feature: Counter projector dispatch

  The CounterProjector proves the read-side dispatch mechanisms the shared
  router must implement identically in every language: every delivered event
  folds into one projection instance, a delivery from an undeclared domain
  folds nothing, and a delivery missing its cover surfaces a coded error.

  Scenario Outline: every delivered event folds into one projection
    Given a counter projection
    When <count> events are delivered in domain "counter"
    Then the projection records <count> events

    Examples:
      | count |
      | 1     |
      | 3     |
      | 5     |

  Scenario: a delivery from an undeclared domain folds nothing
    Given a counter projection
    When 3 events are delivered in domain "inventory"
    Then the projection records nothing

  Scenario: a delivery missing its cover is refused
    Given a counter projection
    When a delivery arrives with no cover
    Then the delivery fails with MISSING_EVENT_BOOK_COVER
