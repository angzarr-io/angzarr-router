@binding
Feature: ABI version

  The binding reports the ABI version of the router-ffi library it loaded, and
  refuses a library whose ABI version drifted from the one it was built for.

  Scenario: the router reports the loaded library's ABI version
    Then the router reports ABI version 2

  Scenario: a drifted router-ffi library is refused
    When the binding checks a library reporting ABI version 1
    Then the check fails naming expected version 2 and actual version 1
