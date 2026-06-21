package io.angzarr.router;

import com.google.protobuf.Message;
import io.angzarr.router.Thunks.ProjectorEventThunk;
import io.angzarr.router.Thunks.ProjectorFinishThunk;
import io.angzarr.router.Thunks.ProjectorUnknownThunk;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Supplier;

/**
 * One projector component's registration: its name, projection-state factory,
 * the domains it folds, per-event fold thunks, and the finisher that carries the
 * cover onto the Projection.
 */
public final class ProjectorDispatch {
  final String name;
  final Supplier<Message.Builder> factory;
  List<String> domains = List.of();
  final Map<String, ProjectorEventThunk> events = new LinkedHashMap<>();
  ProjectorFinishThunk finish;
  ProjectorUnknownThunk unknown;

  public ProjectorDispatch(String name, Supplier<Message.Builder> factory) {
    this.name = name;
    this.factory = factory;
  }

  /** Declares the domains this projector folds. */
  public ProjectorDispatch forDomains(String... domains) {
    this.domains = List.of(domains);
    return this;
  }

  /** Registers the fold thunk for a fully-qualified event type. */
  public ProjectorDispatch onEvent(String fullName, ProjectorEventThunk thunk) {
    events.put(fullName, thunk);
    return this;
  }

  /** Registers the finisher that produces the Projection from the folded state. */
  public ProjectorDispatch finish(ProjectorFinishThunk thunk) {
    this.finish = thunk;
    return this;
  }

  /** Registers an optional observer for events outside the declared set. */
  public ProjectorDispatch onUnknown(ProjectorUnknownThunk thunk) {
    this.unknown = thunk;
    return this;
  }
}
