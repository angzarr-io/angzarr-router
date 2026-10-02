package io.angzarr.router;

import com.google.protobuf.Any;
import java.util.List;
import java.util.Objects;

/**
 * What a fact handler records: the fact (as received, or annotated) followed by the events that
 * flag it, in order. Each recorded event folds into the state the next fact sees. A fact cannot be
 * refused, so there is no way to record nothing.
 */
public record FactRecord(Any fact, List<Any> flags) {

  public FactRecord {
    Objects.requireNonNull(fact, "a fact record carries the fact to record");
    flags = List.copyOf(Objects.requireNonNull(flags, "flags"));
  }

  /** The fact recorded with the given flagging events. */
  public static FactRecord of(Any fact, Any... flags) {
    return new FactRecord(fact, List.of(flags));
  }

  /** The fact recorded as received, with no flags. */
  public static FactRecord of(Any fact) {
    return new FactRecord(fact, List.of());
  }
}
