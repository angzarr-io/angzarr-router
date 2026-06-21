import { clone, create } from "@bufbuild/protobuf";

import { CodedError } from "./codedError";
import { GrpcCode } from "./grpcCode";
import {
  type CommandBook,
  CommandBookSchema,
  PageHeaderSchema,
} from "../gen/io/angzarr/v1/types_pb";

/**
 * The coordinator-supplied next-sequences for command stamping. Sagas and
 * process managers are translators — they stamp emitted commands, they do not
 * rebuild destination state to make decisions.
 */
export class Destinations {
  private readonly sequences: Readonly<Record<string, number>>;

  /** Wraps a domain→next-sequence map (undefined becomes empty). */
  constructor(sequences?: Record<string, number>) {
    this.sequences = sequences ?? {};
  }

  /** The next sequence for a domain, or undefined if none exists. */
  sequenceFor(domain: string): number | undefined {
    return this.sequences[domain];
  }

  /** Whether a sequence exists for the domain. */
  has(domain: string): boolean {
    return Object.prototype.hasOwnProperty.call(this.sequences, domain);
  }

  /** Every domain carrying a sequence (unordered). */
  domains(): string[] {
    return Object.keys(this.sequences);
  }

  /**
   * Returns a copy of `cmd` with every page stamped with the next sequence for
   * `domain`. A domain with no supplied sequence is the coded
   * MISSING_DESTINATION_SEQUENCE.
   */
  stampCommand(cmd: CommandBook, domain: string): CommandBook {
    if (!this.has(domain)) {
      throw new CodedError(
        "MISSING_DESTINATION_SEQUENCE",
        "no sequence for destination domain",
        GrpcCode.InvalidArgument,
        { domain },
      );
    }
    const seq = this.sequences[domain];
    const book = clone(CommandBookSchema, cmd);
    for (const page of book.pages) {
      page.header = create(PageHeaderSchema, {
        sequenceType: { case: "sequence", value: seq },
      });
    }
    return book;
  }
}
