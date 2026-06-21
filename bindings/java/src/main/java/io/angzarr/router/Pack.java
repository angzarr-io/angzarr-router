package io.angzarr.router;

import com.google.protobuf.Any;
import com.google.protobuf.Message;

/** Wraps a message in a google.protobuf.Any using the framework's bare-"/"
 * type-URL convention (NOT the type.googleapis.com prefix). The core keys
 * event/command dispatch on it; generated typed-emit wiring uses it to build an
 * EventBook from the typed events a command handler returns. */
public final class Pack {
  private Pack() {}

  private static final String FRAMEWORK_ANY_PREFIX = "/";

  public static Any pack(Message msg) {
    return Any.newBuilder()
        .setTypeUrl(FRAMEWORK_ANY_PREFIX + msg.getDescriptorForType().getFullName())
        .setValue(msg.toByteString())
        .build();
  }
}
