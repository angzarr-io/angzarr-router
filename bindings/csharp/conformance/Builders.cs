using System;
using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using Test.Counter;

namespace Angzarr.Router.Conformance;

/// <summary>
/// The shared conformance envelopes. Google.Protobuf (C#) has no text-format
/// parser, so — unlike the Java/Rust harnesses that parse
/// conformance/fixtures/*.txtpb — these are built BY FIELD, byte-equivalent to
/// those skeletons (each is an orthogonal envelope wrapping an empty inner
/// message; the salient field is set from the scenario). This mirrors how the Go
/// binding constructs its no-skeleton envelopes. The behaviour asserted is the
/// same cross-language contract.
/// </summary>
public static class Builders
{
    /// <summary>The framework's canonical bare-"/" type-URL prefix (the core
    /// keys dispatch on the suffix, so the prefix is immaterial).</summary>
    public static string TypeUrl(string fq) => "/" + fq;

    private static Any AnyOf(string fq, ByteString value) =>
        new() { TypeUrl = TypeUrl(fq), Value = value };

    private static Any AnyEmpty(string fq) => new() { TypeUrl = TypeUrl(fq) };

    // --- commands -----------------------------------------------------------

    /// <summary>The IncreaseBy envelope (command_increase.txtpb) with n set.</summary>
    public static ContextualCommand IncreaseCommand(int n)
    {
        var inner = new IncreaseBy { N = (uint)n };
        return new ContextualCommand
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages =
                {
                    new CommandPage
                    {
                        Command = AnyOf("test.counter.IncreaseBy", inner.ToByteString()),
                    },
                },
            },
        };
    }

    /// <summary>An increase command with parent linkage stamped on its cover.</summary>
    public static ContextualCommand IncreaseCommandWithLinkage(int n)
    {
        var cc = IncreaseCommand(n);
        cc.Command.Cover.Ext = ParentLinkage();
        return cc;
    }

    public static ContextualCommand FailHardCommand() =>
        new()
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new CommandPage { Command = AnyEmpty("test.counter.FailHard") } },
            },
        };

    public static ContextualCommand UnhandledCommand() =>
        new()
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new CommandPage { Command = AnyEmpty("test.counter.Reserve") } },
            },
        };

    /// <summary>Wraps a rejection Notification for fqCommand into a
    /// ContextualCommand — the core detects the notification type and takes the
    /// compensation path.</summary>
    public static ContextualCommand RejectionCommand(string fqCommand)
    {
        var notification = RejectionNotificationFor(fqCommand, "counter");
        return new ContextualCommand
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new CommandPage { Command = Pack.Wrap(notification) } },
            },
        };
    }

    // --- envelope-guard negatives (one structural field cleared) ------------

    public static ContextualCommand CommandMissingBook()
    {
        var cc = IncreaseCommand(1);
        cc.Command = null;
        return cc;
    }

    public static ContextualCommand CommandMissingPage()
    {
        var cc = IncreaseCommand(1);
        cc.Command.Pages.Clear();
        return cc;
    }

    public static ContextualCommand CommandMissingPayload()
    {
        var cc = IncreaseCommand(1);
        cc.Command.Pages[0].Command = null;
        return cc;
    }

    /// <summary>An opaque fill-only ext stamped on a command's cover, used to
    /// prove ext propagation onto emitted events.</summary>
    public static Any ParentLinkage() =>
        AnyOf("test.counter.Parent", ByteString.CopyFrom(new byte[] { 1, 2, 3 }));

    // --- prior history ------------------------------------------------------

    /// <summary>Replays the Increased skeleton at sequences 0..n-1 (null if 0).</summary>
    public static EventBook? PriorIncreases(int n)
    {
        if (n == 0)
        {
            return null;
        }
        var book = new EventBook { NextSequence = (uint)n };
        for (var i = 0; i < n; i++)
        {
            book.Pages.Add(IncreasedPageAt(i));
        }
        return book;
    }

    /// <summary>One Increased page whose payload is an undecodable varint
    /// (PERSISTED_EVENT_CORRUPT on fold).</summary>
    public static EventBook CorruptHistory()
    {
        var page = IncreasedPageAt(0);
        page.Event.Value = ByteString.CopyFrom(new byte[] { 0xff, 0xff, 0xff });
        return new EventBook { Pages = { page }, NextSequence = 1 };
    }

    /// <summary>Seeds count 10 at sequence 10, plus a covered page (10, skipped)
    /// and an uncovered page (11, applied) — a rebuild observes 11.</summary>
    public static EventBook SnapshotHistory() =>
        new()
        {
            Snapshot = new Snapshot
            {
                Sequence = 10,
                State = Pack.Wrap(new CounterState { Count = 10 }),
            },
            Pages = { IncreasedPageAt(10), IncreasedPageAt(11) },
            NextSequence = 12,
        };

    /// <summary>Rewrites every command and event Any type URL of
    /// <paramref name="book"/>'s pages to <paramref name="prefix"/> + the
    /// fully-qualified name.</summary>
    public static EventBook? WithTypeUrlPrefix(EventBook? book, string prefix)
    {
        if (book == null)
        {
            return null;
        }
        var copy = book.Clone();
        foreach (var page in copy.Pages)
        {
            if (page.Event != null)
            {
                page.Event.TypeUrl = prefix + TypeNames.FromUrl(page.Event.TypeUrl);
            }
        }
        return copy;
    }

    /// <summary>A copy of <paramref name="cc"/> whose command Any type URLs (and
    /// prior history's event type URLs) carry <paramref name="prefix"/>.</summary>
    public static ContextualCommand WithTypeUrlPrefix(ContextualCommand cc, string prefix)
    {
        var copy = cc.Clone();
        if (copy.Command != null)
        {
            foreach (var page in copy.Command.Pages)
            {
                if (page.Command != null)
                {
                    page.Command.TypeUrl = prefix + TypeNames.FromUrl(page.Command.TypeUrl);
                }
            }
        }
        copy.Events = WithTypeUrlPrefix(copy.Events, prefix);
        return copy;
    }

    /// <summary>One Increased event page stamped with a sequence.</summary>
    public static EventPage IncreasedPageAt(int seq) =>
        new()
        {
            Event = AnyEmpty("test.counter.Increased"),
            Header = new PageHeader { Sequence = (uint)seq },
        };

    // --- saga / process-manager shared fixtures -----------------------------

    /// <summary>The one-page Reserve command the saga and PM emit for
    /// "inventory".</summary>
    public static CommandBook ReserveCommand() =>
        new()
        {
            Cover = new Cover { Domain = "inventory" },
            Pages = { new CommandPage { Command = AnyEmpty("test.counter.Reserve") } },
        };

    /// <summary>A single empty fact-event book the compensators inject.</summary>
    public static EventBook OneFact() => new() { Pages = { new EventPage() } };

    private static Notification RejectionNotificationFor(
        string fqCommand,
        string domain,
        string code = "",
        string message = ""
    )
    {
        var rejection = new RejectionNotification
        {
            RejectedCommand = new CommandBook
            {
                Cover = new Cover { Domain = domain },
                Pages = { new CommandPage { Command = AnyEmpty(fqCommand) } },
            },
            RejectionReason = message,
            Code = code,
        };
        return new Notification { Payload = Pack.Wrap(rejection) };
    }

    // --- saga dispatch requests ---------------------------------------------

    /// <summary>A saga source of one fq event in "order", at sequence seq when
    /// given.</summary>
    public static SagaHandleRequest SagaEventSource(string fq, uint? seq)
    {
        var page = new EventPage { Event = AnyEmpty(fq) };
        if (seq is uint s)
        {
            page.Header = new PageHeader { Sequence = s };
        }
        return new SagaHandleRequest
        {
            Source = new EventBook
            {
                Cover = new Cover { Domain = "order" },
                Pages = { page },
            },
        };
    }

    /// <summary>A saga source of one Increased event of order root label at
    /// sequence seq.</summary>
    public static SagaHandleRequest SagaRootedSource(string label, uint seq)
    {
        var req = SagaEventSource("test.counter.Increased", seq);
        req.Source.Cover = CoverOf("order", label);
        return req;
    }

    /// <summary>count bytes counting up from first.</summary>
    private static ByteString ByteRun(int first, int count)
    {
        var b = new byte[count];
        for (var i = 0; i < count; i++)
        {
            b[i] = (byte)(first + i);
        }
        return ByteString.CopyFrom(b);
    }

    /// <summary>The parity command: cover "inventory", root bytes 10..1f,
    /// correlation "corr-1"; one page whose command is "/example.Foo" carrying
    /// 01020304.</summary>
    public static CommandBook ParityCommand() =>
        new()
        {
            Cover = new Cover
            {
                Domain = "inventory",
                Root = new UUID { Value = ByteRun(0x10, 16) },
                CorrelationId = "corr-1",
            },
            Pages =
            {
                new CommandPage
                {
                    Command = new Any { TypeUrl = "/example.Foo", Value = ByteRun(1, 4) },
                },
            },
        };

    /// <summary>The parity source: one Increased event at sequence seq under
    /// cover "order", root bytes 00..0f, correlation "corr-1".</summary>
    public static SagaHandleRequest ParitySource(uint seq)
    {
        var req = SagaEventSource("test.counter.Increased", seq);
        req.Source.Cover = new Cover
        {
            Domain = "order",
            Root = new UUID { Value = ByteRun(0x00, 16) },
            CorrelationId = "corr-1",
        };
        return req;
    }

    public static SagaHandleRequest SagaRejectionSource(string fqCommand)
    {
        var notification = RejectionNotificationFor(fqCommand, "inventory");
        return new SagaHandleRequest
        {
            Source = new EventBook
            {
                Cover = new Cover { Domain = "order" },
                Pages = { new EventPage { Event = Pack.Wrap(notification) } },
            },
        };
    }

    public static SagaHandleRequest SagaSourceNoPages() => new() { Source = new EventBook() };

    public static SagaHandleRequest SagaRequestNoSource() => new();

    // --- projector deliveries -----------------------------------------------

    public static EventPage IncreasedEventPage() =>
        new() { Event = AnyEmpty("test.counter.Increased") };

    public static EventBook DeliveryBook(string domain, int n)
    {
        var book = new EventBook { Cover = new Cover { Domain = domain } };
        for (var i = 0; i < n; i++)
        {
            book.Pages.Add(IncreasedEventPage());
        }
        return book;
    }

    public static EventBook DeliveryNoCover()
    {
        var book = DeliveryBook("counter", 1);
        book.Cover = null;
        return book;
    }

    // --- process-manager triggers -------------------------------------------

    /// <summary>A PM trigger of the fq event pages in domain, the newest at
    /// sequence newestSeq when given, over the PM's prior state.</summary>
    public static ProcessManagerHandleRequest PmTrigger(
        string domain,
        IReadOnlyList<string> fqs,
        EventBook? state,
        uint? newestSeq
    )
    {
        var trigger = new EventBook { Cover = new Cover { Domain = domain } };
        foreach (var fq in fqs)
        {
            trigger.Pages.Add(new EventPage { Event = AnyEmpty(fq) });
        }
        if (newestSeq is uint seq && trigger.Pages.Count > 0)
        {
            trigger.Pages[^1].Header = new PageHeader { Sequence = seq };
        }
        var req = new ProcessManagerHandleRequest { Trigger = trigger };
        if (state != null)
        {
            req.ProcessState = state;
        }
        return req;
    }

    /// <summary>A PM request whose trigger (in the order PM's own domain) is a
    /// Compensate for an executed test.counter.&lt;command&gt;.</summary>
    public static ProcessManagerHandleRequest PmCompensate(string command) =>
        new()
        {
            Trigger = new EventBook
            {
                Cover = new Cover { Domain = "order-pm" },
                Pages =
                {
                    new EventPage
                    {
                        Event = Pack.Wrap(
                            new Notification { Payload = CompensatePayload(command) }
                        ),
                    },
                },
            },
        };

    /// <summary>The Compensate payload for an executed
    /// test.counter.&lt;command&gt;.</summary>
    public static Any CompensatePayload(string command) =>
        Pack.Wrap(
            new Compensate
            {
                CommandType = "test.counter." + command,
                Sequences = { 0 },
                Reason = "aborted",
            }
        );

    public static EventBook PmStateOf(int n)
    {
        var book = new EventBook();
        for (var i = 0; i < n; i++)
        {
            book.Pages.Add(IncreasedEventPage());
        }
        return book;
    }

    public static ProcessManagerHandleRequest PmRejection(string fqCommand) =>
        PmRejectionWith(fqCommand, "", "");

    /// <summary>A rejection of fqCommand whose RejectionNotification carries
    /// code and message.</summary>
    public static ProcessManagerHandleRequest PmRejectionWith(
        string fqCommand,
        string code,
        string message
    )
    {
        var notification = RejectionNotificationFor(fqCommand, "inventory", code, message);
        return new ProcessManagerHandleRequest
        {
            Trigger = new EventBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new EventPage { Event = Pack.Wrap(notification) } },
            },
        };
    }

    /// <summary>A process state of n Increased pages owned by (covered with)
    /// the given PM domain.</summary>
    public static EventBook PmStateIn(string owner, int n)
    {
        var book = PmStateOf(n);
        book.Cover = new Cover { Domain = owner };
        return book;
    }

    /// <summary>A rejection of fqCommand addressed to the issuing PM's domain:
    /// the trigger cover is the issuer, and the rejected command's
    /// angzarr_deferred header names the issuer as its source.</summary>
    public static ProcessManagerHandleRequest PmIssuedRejection(string fqCommand, string issuer)
    {
        var rejection = new RejectionNotification
        {
            RejectedCommand = new CommandBook
            {
                Cover = new Cover { Domain = "inventory" },
                Pages =
                {
                    new CommandPage
                    {
                        Command = AnyEmpty(fqCommand),
                        Header = new PageHeader
                        {
                            AngzarrDeferred = new AngzarrDeferredSequence
                            {
                                Source = new Cover { Domain = issuer },
                            },
                        },
                    },
                },
            },
        };
        var notification = new Notification { Payload = Pack.Wrap(rejection) };
        return new ProcessManagerHandleRequest
        {
            Trigger = new EventBook
            {
                Cover = new Cover { Domain = issuer },
                Pages = { new EventPage { Event = Pack.Wrap(notification) } },
            },
        };
    }

    public static ProcessManagerHandleRequest PmNoTrigger() => new();

    public static ProcessManagerHandleRequest PmEmptyTrigger() =>
        new() { Trigger = new EventBook() };

    // --- compensation routing (compensation.feature) ------------------------

    /// <summary>A business response of one header-less test.counter.&lt;name&gt;
    /// event page.</summary>
    public static BusinessResponse OneEvent(string name) =>
        new()
        {
            Events = new EventBook
            {
                Pages = { new EventPage { Event = AnyEmpty("test.counter." + name) } },
            },
        };

    /// <summary>A Notification command (bare-"/" type URL) wrapping payload,
    /// addressed to domain, over one prior event whose next sequence is
    /// nextSequence when given.</summary>
    private static ContextualCommand NotificationCommand(
        string domain,
        Any payload,
        uint? nextSequence
    )
    {
        var cc = new ContextualCommand
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = domain },
                Pages =
                {
                    new CommandPage { Command = Pack.Wrap(new Notification { Payload = payload }) },
                },
            },
        };
        if (nextSequence is uint next)
        {
            cc.Events = new EventBook
            {
                NextSequence = next,
                Pages =
                {
                    new EventPage
                    {
                        Header = new PageHeader { Sequence = next == 0 ? 0 : next - 1 },
                        Event = AnyEmpty("test.counter.Unrelated"),
                    },
                },
            };
        }
        return cc;
    }

    /// <summary>The rejection of a test.counter.&lt;command&gt; sent to
    /// targetDomain, delivered to the payment aggregate.</summary>
    public static ContextualCommand RejectionSentTo(
        string command,
        string targetDomain,
        uint? nextSequence
    ) => RejectionWith(command, targetDomain, nextSequence, "", "");

    /// <summary>RejectionSentTo whose RejectionNotification carries code (its
    /// machine code) and message (its rejection_reason).</summary>
    public static ContextualCommand RejectionWith(
        string command,
        string targetDomain,
        uint? nextSequence,
        string code,
        string message
    )
    {
        var rejection = new RejectionNotification
        {
            RejectedCommand = new CommandBook
            {
                Cover = new Cover { Domain = targetDomain },
                Pages = { new CommandPage { Command = AnyEmpty("test.counter." + command) } },
            },
            RejectionReason = message,
            Code = code,
        };
        return NotificationCommand("payment", Pack.Wrap(rejection), nextSequence);
    }

    /// <summary>A Compensate for an executed test.counter.&lt;command&gt;,
    /// delivered to the inventory aggregate.</summary>
    public static ContextualCommand CompensateFor(string command) =>
        NotificationCommand("inventory", CompensatePayload(command), null);

    // --- facts, replay and handler context (context.feature) ----------------

    private static readonly byte[] NamespaceOid = Guid.Parse("6ba7b812-9dad-11d1-80b4-00c04fd430c8")
        .ToByteArray(bigEndian: true);

    /// <summary>The root bytes for a label: UUID v5 in the OID namespace (RFC
    /// 4122 byte order), byte-identical to every other language's
    /// fixtures.</summary>
    public static byte[] RootOf(string label)
    {
        var name = System.Text.Encoding.UTF8.GetBytes(label);
        var input = new byte[NamespaceOid.Length + name.Length];
        NamespaceOid.CopyTo(input, 0);
        name.CopyTo(input, NamespaceOid.Length);
        var hash = System.Security.Cryptography.SHA1.HashData(input);
        var uuid = new byte[16];
        Array.Copy(hash, uuid, 16);
        uuid[6] = (byte)((uuid[6] & 0x0f) | 0x50);
        uuid[8] = (byte)((uuid[8] & 0x3f) | 0x80);
        return uuid;
    }

    /// <summary>A cover in domain with the root for label.</summary>
    public static Cover CoverOf(string domain, string label) =>
        new()
        {
            Domain = domain,
            Root = new UUID { Value = ByteString.CopyFrom(RootOf(label)) },
        };

    /// <summary>A FactRequest of `facts` pages of test.counter.&lt;fact&gt; in
    /// "ledger" over `prior` Increased events.</summary>
    public static FactRequest FactsOver(string fact, int facts, int prior)
    {
        var book = new EventBook { Cover = new Cover { Domain = "ledger" } };
        for (var i = 0; i < facts; i++)
        {
            book.Pages.Add(new EventPage { Event = AnyEmpty("test.counter." + fact) });
        }
        var history = new EventBook { NextSequence = (uint)prior };
        for (var i = 0; i < prior; i++)
        {
            history.Pages.Add(IncreasedPageAt(i));
        }
        return new FactRequest { Facts = book, PriorEvents = history };
    }

    /// <summary>A ReplayRequest of <paramref name="events"/> Increased events
    /// at sequences 0..., with no snapshot.</summary>
    public static ReplayRequest EventsReplayOf(int events)
    {
        var req = new ReplayRequest();
        for (var i = 0; i < events; i++)
        {
            req.Events.Add(IncreasedPageAt(i));
        }
        return req;
    }

    /// <summary>A ReplayRequest: a snapshot of count at sequence 1, then
    /// `events` Increased events at sequences 2...</summary>
    public static ReplayRequest ReplayOf(int count, int events)
    {
        var req = new ReplayRequest
        {
            BaseSnapshot = new Snapshot
            {
                Sequence = 1,
                State = Pack.Wrap(new CounterState { Count = (uint)count }),
            },
        };
        for (var i = 0; i < events; i++)
        {
            req.Events.Add(IncreasedPageAt(2 + i));
        }
        return req;
    }

    /// <summary>An IncreaseBy command for the ledger root label.</summary>
    public static ContextualCommand LedgerCommand(string label) =>
        new()
        {
            Command = new CommandBook
            {
                Cover = CoverOf("ledger", label),
                Pages =
                {
                    new CommandPage
                    {
                        Command = AnyOf(
                            "test.counter.IncreaseBy",
                            new IncreaseBy { N = 1 }.ToByteString()
                        ),
                    },
                },
            },
        };

    /// <summary>The parent linkage the ledger sets on its own events.</summary>
    public static Any LedgerLinkage() =>
        AnyOf("test.counter.Parent", ByteString.CopyFrom(new byte[] { 4, 5, 6 }));

    /// <summary>An IncreaseBy command for the ledger root label on behalf of a
    /// parent.</summary>
    public static ContextualCommand LedgerCommandWithLinkage(string label)
    {
        var cc = LedgerCommand(label);
        cc.Command.Cover.Ext = ParentLinkage();
        return cc;
    }

    /// <summary>An Increased trigger from "counter" root label at sequence
    /// seq.</summary>
    public static ProcessManagerHandleRequest ReservingTrigger(string label, int seq) =>
        new()
        {
            Trigger = new EventBook
            {
                Cover = CoverOf("counter", label),
                Pages = { IncreasedPageAt(seq) },
            },
        };

    /// <summary>The rejection of a Reserve sent to targetDomain, delivered to
    /// the reserving process-manager's own domain at sequence seq.</summary>
    public static ProcessManagerHandleRequest ReservingRejection(string targetDomain, int seq)
    {
        var rejection = new RejectionNotification
        {
            RejectedCommand = new CommandBook
            {
                Cover = new Cover { Domain = targetDomain },
                Pages = { new CommandPage { Command = AnyEmpty("test.counter.Reserve") } },
            },
        };
        return new ProcessManagerHandleRequest
        {
            Trigger = new EventBook
            {
                Cover = new Cover { Domain = "reserving-pm" },
                Pages =
                {
                    new EventPage
                    {
                        Header = new PageHeader { Sequence = (uint)seq },
                        Event = Pack.Wrap(new Notification { Payload = Pack.Wrap(rejection) }),
                    },
                },
            },
        };
    }

    /// <summary>A book of Increased events of "counter" root label at the
    /// given sequences.</summary>
    public static EventBook TrackedBook(string label, params int[] sequences)
    {
        var book = new EventBook { Cover = CoverOf("counter", label) };
        foreach (var seq in sequences)
        {
            book.Pages.Add(IncreasedPageAt(seq));
        }
        return book;
    }
}
