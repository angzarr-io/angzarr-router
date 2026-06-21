package io.angzarr.router;

import io.angzarr.CommandBook;
import io.angzarr.CommandPage;
import io.angzarr.PageHeader;
import java.util.List;
import java.util.Map;

/**
 * The coordinator-supplied next-sequences for command stamping. Sagas and
 * process managers are translators — they stamp emitted commands, they do not
 * rebuild destination state to make decisions.
 */
public final class Destinations {
  private final Map<String, Integer> sequences;

  /** Wraps a domain→next-sequence map (null becomes empty). */
  public Destinations(Map<String, Integer> sequences) {
    this.sequences = sequences == null ? Map.of() : sequences;
  }

  /** Returns the next sequence for a domain, or null if none exists. */
  public Integer sequenceFor(String domain) {
    return sequences.get(domain);
  }

  /** Reports whether a sequence exists for the domain. */
  public boolean has(String domain) {
    return sequences.containsKey(domain);
  }

  /** Every domain carrying a sequence (unordered). */
  public List<String> domains() {
    return List.copyOf(sequences.keySet());
  }

  /**
   * Returns a copy of {@code cmd} with every page stamped with the next sequence
   * for {@code domain}. A domain with no supplied sequence is the coded {@code
   * MISSING_DESTINATION_SEQUENCE} (check output_domains config).
   */
  public CommandBook stampCommand(CommandBook cmd, String domain) {
    Integer seq = sequences.get(domain);
    if (seq == null) {
      throw new CodedError(
          "MISSING_DESTINATION_SEQUENCE",
          "no sequence for destination domain",
          GrpcCode.INVALID_ARGUMENT,
          Map.of("domain", domain));
    }
    CommandBook.Builder book = cmd.toBuilder();
    for (CommandPage.Builder page : book.getPagesBuilderList()) {
      page.setHeader(PageHeader.newBuilder().setSequence(seq));
    }
    return book.build();
  }
}
