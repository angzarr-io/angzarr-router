package io.angzarr.router;

import com.google.protobuf.Message;
import io.angzarr.router.Thunks.ApplierThunk;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.function.Supplier;

/**
 * Folds a component's prior events (and optional snapshot) into a state builder
 * before a command runs. The factory produces a fresh state builder; appliers
 * mutate it page by page.
 */
public final class Rebuilder {
  final Supplier<Message.Builder> factory;
  final Map<String, ApplierThunk> appliers = new LinkedHashMap<>();
  ApplierThunk snapshot;

  /** Starts a rebuilder from a zero-state builder factory (e.g. {@code
   * CounterState::newBuilder}). */
  public Rebuilder(Supplier<Message.Builder> factory) {
    this.factory = factory;
  }

  /** Registers an applier for one fully-qualified event type. */
  public Rebuilder apply(String fullName, ApplierThunk thunk) {
    appliers.put(fullName, thunk);
    return this;
  }

  /** Registers the snapshot loader that seeds state before pages. */
  public Rebuilder withSnapshot(ApplierThunk thunk) {
    snapshot = thunk;
    return this;
  }
}
