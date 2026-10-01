//go:build ffirouter

package conformance

import (
	"errors"
	"fmt"
	"os"

	"google.golang.org/protobuf/encoding/prototext"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
	counter "github.com/angzarr-io/angzarr-router/bindings/go/gen/test/counter"
)

// The shared conformance fixtures: the same orthogonal envelope skeletons
// the Rust harness parses. Every builder PARSES the skeleton first, then
// sets the scenario's data BY FIELD on the structured message — the
// textproto is never string-templated or altered before parsing.
const fixturesDir = "../../../conformance/fixtures"

// The framework's canonical type-URL prefix is a bare "/" (router
// TYPE_URL_PREFIX); angzarr-produced URLs take that form, not the
// type.googleapis.com Any default. The core matches prefix-agnostically,
// but the binding emits the canonical form. (The google.rpc.ErrorInfo
// detail in the error model is the one exception — the ABI pins it to the
// type.googleapis.com form the core string-matches; see api.go.)
const typeURLPrefix = "/"

func typeURL(fq string) string { return typeURLPrefix + fq }

// loadSkeleton reads a .txtpb skeleton and parses it into m, expanding Any
// payloads via the global proto registry (the generated packages register
// their types on import).
func loadSkeleton(name string, m proto.Message) {
	b, err := os.ReadFile(fixturesDir + "/" + name)
	if err != nil {
		panic(fmt.Sprintf("read fixture %s: %v", name, err))
	}
	if err := prototext.Unmarshal(b, m); err != nil {
		panic(fmt.Sprintf("parse fixture %s: %v", name, err))
	}
}

// increaseCommand parses the IncreaseBy skeleton, then sets n on the inner
// message by field (decode the Any payload, set N, re-encode) — never by
// editing the textproto.
func increaseCommand(n uint32) *pb.ContextualCommand {
	cc := &pb.ContextualCommand{}
	loadSkeleton("command_increase.txtpb", cc)
	any := innerCommandAny(cc)
	var inner counter.IncreaseBy
	if err := proto.Unmarshal(any.Value, &inner); err != nil {
		panic(fmt.Sprintf("decode IncreaseBy skeleton: %v", err))
	}
	inner.N = n
	any.Value = mustMarshal(&inner)
	return cc
}

// failHardCommand parses the FailHard skeleton (no scenario data).
func failHardCommand() *pb.ContextualCommand {
	cc := &pb.ContextualCommand{}
	loadSkeleton("command_failhard.txtpb", cc)
	return cc
}

// unhandledCommand parses the Reserve skeleton — a command with no
// registered handler (drives NO_HANDLER_REGISTERED before rebuild).
func unhandledCommand() *pb.ContextualCommand {
	cc := &pb.ContextualCommand{}
	loadSkeleton("command_unhandled.txtpb", cc)
	return cc
}

// parentLinkage is an opaque fill-only ext stamped on a command's cover,
// used to prove ext propagation onto emitted events.
func parentLinkage() *anypb.Any {
	return &anypb.Any{TypeUrl: typeURL("test.counter.Parent"), Value: []byte{1, 2, 3}}
}

// increaseCommandWithLinkage sets parent linkage on a parsed command's cover.
func increaseCommandWithLinkage(n uint32) *pb.ContextualCommand {
	cc := increaseCommand(n)
	cc.Command.Cover.Ext = parentLinkage()
	return cc
}

// rejectionCommand wraps a rejection Notification for fqCommand into a
// ContextualCommand, routed through the same dispatch entry — the core
// detects the notification type and takes the compensation path. Built by
// field; the envelope nests Notification -> RejectionNotification -> the
// rejected book.
func rejectionCommand(fqCommand string) *pb.ContextualCommand {
	cover := func() *pb.Cover { return &pb.Cover{Domain: "counter"} }
	rejection := &pb.RejectionNotification{
		RejectedCommand: &pb.CommandBook{
			Cover: cover(),
			Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{
				Command: &anypb.Any{TypeUrl: typeURL(fqCommand)},
			}}},
		},
	}
	notification := &pb.Notification{
		Payload: &anypb.Any{
			TypeUrl: typeURL("io.angzarr.v1.RejectionNotification"),
			Value:   mustMarshal(rejection),
		},
	}
	return &pb.ContextualCommand{
		Command: &pb.CommandBook{
			Cover: cover(),
			Pages: []*pb.CommandPage{{Payload: &pb.CommandPage_Command{
				Command: &anypb.Any{
					TypeUrl: typeURL("io.angzarr.v1.Notification"),
					Value:   mustMarshal(notification),
				},
			}}},
		},
	}
}

// Envelope-guard negatives: a well-formed parsed command with exactly one
// structural field cleared, so the guard fires regardless of the rest.

func commandMissingBook() *pb.ContextualCommand {
	cc := increaseCommand(1)
	cc.Command = nil
	return cc
}

func commandMissingPage() *pb.ContextualCommand {
	cc := increaseCommand(1)
	cc.Command.Pages = nil
	return cc
}

func commandMissingPayload() *pb.ContextualCommand {
	cc := increaseCommand(1)
	cc.Command.Pages[0].Payload = nil
	return cc
}

// priorIncreases replays the parsed Increased skeleton at consecutive
// sequences 0..n-1, with the next sequence the core derives.
func priorIncreases(n uint32) *pb.EventBook {
	if n == 0 {
		return nil
	}
	pages := make([]*pb.EventPage, n)
	for i := range pages {
		pages[i] = increasedPageAt(uint32(i))
	}
	return &pb.EventBook{Pages: pages, NextSequence: n}
}

// corruptHistory is one parsed Increased page whose payload is overwritten
// with an undecodable varint, so the fold fails (PERSISTED_EVENT_CORRUPT).
func corruptHistory() *pb.EventBook {
	page := increasedPageAt(0)
	page.GetEvent().Value = []byte{0xff, 0xff, 0xff}
	return &pb.EventBook{Pages: []*pb.EventPage{page}, NextSequence: 1}
}

// snapshotHistory seeds count 10 at sequence 10, plus a covered page (10,
// skipped) and an uncovered page (11, applied) — a rebuild observes 11.
func snapshotHistory() *pb.EventBook {
	return &pb.EventBook{
		Snapshot: &pb.Snapshot{
			Sequence: 10,
			State: &anypb.Any{
				TypeUrl: typeURL("test.counter.CounterState"),
				Value:   mustMarshal(&counter.CounterState{Count: 10}),
			},
		},
		Pages:        []*pb.EventPage{increasedPageAt(10), increasedPageAt(11)},
		NextSequence: 12,
	}
}

// increasedPageAt parses the Increased event skeleton and stamps a sequence.
func increasedPageAt(seq uint32) *pb.EventPage {
	page := &pb.EventPage{}
	loadSkeleton("event_increased.txtpb", page)
	page.Header = &pb.PageHeader{SequenceType: &pb.PageHeader_Sequence{Sequence: seq}}
	return page
}

func innerCommandAny(cc *pb.ContextualCommand) *anypb.Any {
	return cc.GetCommand().GetPages()[0].GetCommand()
}

func mustMarshal(m proto.Message) []byte {
	b, err := proto.Marshal(m)
	if err != nil {
		panic(fmt.Sprintf("marshal %T: %v", m, err))
	}
	return b
}

// sequenceHeader is a page header carrying an explicit sequence.
func sequenceHeader(seq uint32) *pb.PageHeader {
	return &pb.PageHeader{SequenceType: &pb.PageHeader_Sequence{Sequence: seq}}
}

// assertDeferred checks every page of cmd is stamped angzarr_deferred from
// the triggering page: source cover domain, source_seq and command_index.
func assertDeferred(cmd *pb.CommandBook, sourceDomain string, seq, index int) error {
	if len(cmd.GetPages()) == 0 {
		return errors.New("command carries no pages")
	}
	for i, page := range cmd.GetPages() {
		d := page.GetHeader().GetAngzarrDeferred()
		if d == nil {
			return fmt.Errorf("command page %d is not deferred (header %v)", i, page.GetHeader())
		}
		if d.GetSourceSeq() != uint32(seq) {
			return fmt.Errorf("page %d source_seq = %d, want %d", i, d.GetSourceSeq(), seq)
		}
		if d.GetCommandIndex() != uint32(index) {
			return fmt.Errorf("page %d command_index = %d, want %d", i, d.GetCommandIndex(), index)
		}
		if got := d.GetSource().GetDomain(); got != sourceDomain {
			return fmt.Errorf("page %d source domain = %q, want %q", i, got, sourceDomain)
		}
	}
	return nil
}

// withTypeURLPrefix rewrites every Any type URL of cc's command pages and
// prior-history pages to prefix + the fully-qualified name.
func withTypeURLPrefix(cc *pb.ContextualCommand, prefix string) *pb.ContextualCommand {
	rewrite := func(a *anypb.Any) {
		if a != nil {
			a.TypeUrl = prefix + fqFromURL(a.TypeUrl)
		}
	}
	for _, page := range cc.GetCommand().GetPages() {
		rewrite(page.GetCommand())
	}
	for _, page := range cc.GetEvents().GetPages() {
		rewrite(page.GetEvent())
	}
	return cc
}

// compensatePayload is the Compensate Any for an executed fqCommand.
func compensatePayload(fqCommand string) *anypb.Any {
	return &anypb.Any{
		TypeUrl: typeURL("io.angzarr.v1.Compensate"),
		Value: mustMarshal(&pb.Compensate{
			CommandType: fqCommand,
			Sequences:   []uint32{0},
			Reason:      "aborted",
		}),
	}
}
