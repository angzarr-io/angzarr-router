@binding
Feature: Router lifecycle

  Closing a router releases the native router exactly once: a second close is
  a no-op, and a dispatch or registration on a closed router fails with a
  coded error instead of touching released memory.

  Scenario: closing a router twice is safe
    Given a binding router with a counter aggregate
    When the router is closed twice
    Then dispatching through the router fails with ROUTER_CLOSED

  Scenario: a dispatch after close fails cleanly
    Given a binding router with a counter aggregate
    When the router is closed
    Then dispatching through the router fails with ROUTER_CLOSED

  Scenario: a registration after close fails cleanly
    Given a binding router with a counter aggregate
    When the router is closed
    Then registering on the router fails with ROUTER_CLOSED
