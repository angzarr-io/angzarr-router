package io.angzarr.router;

import java.util.List;

/**
 * The declared output domains (command targets) of one saga or process manager, in declaration
 * order. Emitted commands carry no destination sequence: the router stamps their angzarr_deferred
 * provenance, so a handler returns commands unstamped.
 */
public final class Destinations {
  private final List<String> domains;

  /** Wraps the declared output domains (null becomes none). */
  public Destinations(List<String> domains) {
    this.domains = domains == null ? List.of() : List.copyOf(domains);
  }

  /** Reports whether the domain is a declared output domain. */
  public boolean has(String domain) {
    return domains.contains(domain);
  }

  /** The declared output domains, in declaration order. */
  public List<String> domains() {
    return domains;
  }
}
