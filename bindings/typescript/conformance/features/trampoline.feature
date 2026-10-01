@binding
Feature: Callback trampoline

  Every failing host callback carries a google.rpc.Status payload, including
  one the core issues for a host context with no live dispatch session.

  Scenario: a callback for an unregistered host context carries an UNHANDLED status
    When the host callback runs for an unregistered host context
    Then the callback fails INTERNAL with UNHANDLED_HANDLER_ERROR
