//go:build ffirouter

package ffirouter

/*
// Cross-platform link to the router-ffi cdylib: cargo emits
// libangzarr_router_ffi.so on Linux, .dylib on macOS, and
// angzarr_router_ffi.dll (+ import lib) on Windows from crate-type=cdylib.
// Dynamic linking keeps the link flags identical across the three OSes —
// the shared lib already resolves its own native dependencies — instead of
// enumerating Rust std's per-OS native libs (Linux: -lgcc_s -lutil -lrt
// -lpthread -lm -ldl -lc). The rpath points the loader at the in-repo build
// dir so `go test` needs no LD_LIBRARY_PATH/DYLD_LIBRARY_PATH; Windows has
// no rpath, so the justfile recipe puts the .dll on PATH. Override the
// search+rpath dir via CGO_LDFLAGS (justfile / ANGZARR_ROUTER_LIB).
#cgo linux LDFLAGS: -L${SRCDIR}/../../target/debug -langzarr_router_ffi -Wl,-rpath,${SRCDIR}/../../target/debug
#cgo darwin LDFLAGS: -L${SRCDIR}/../../target/debug -langzarr_router_ffi -Wl,-rpath,${SRCDIR}/../../target/debug
#cgo windows LDFLAGS: -L${SRCDIR}/../../target/debug -langzarr_router_ffi
#include <stdint.h>
#include <stddef.h>

typedef struct { uint8_t* data; size_t len; } angzarr_buf;
typedef int32_t (*angzarr_cb)(void*, uint64_t, const uint8_t*, size_t,
                              const uint8_t*, size_t, const uint8_t*, size_t,
                              angzarr_buf*);

uint32_t angzarr_abi_version(void);
uint8_t* angzarr_buf_alloc(size_t);
void     angzarr_buf_release(uint8_t*, size_t);
void*    angzarr_router_new(void);
void     angzarr_router_free(void*);
int32_t  angzarr_router_register_aggregate(void*, const uint8_t*, size_t, angzarr_cb);
int32_t  angzarr_router_dispatch(void*, void*, const uint8_t*, size_t, angzarr_buf*);
int32_t  angzarr_router_register_projector(void*, const uint8_t*, size_t, angzarr_cb);
int32_t  angzarr_router_dispatch_projector(void*, void*, const uint8_t*, size_t, angzarr_buf*);
int32_t  angzarr_router_register_saga(void*, const uint8_t*, size_t, angzarr_cb);
int32_t  angzarr_router_dispatch_saga(void*, void*, const uint8_t*, size_t, angzarr_buf*);
int32_t  angzarr_router_register_process_manager(void*, const uint8_t*, size_t, angzarr_cb);
int32_t  angzarr_router_dispatch_process_manager(void*, void*, const uint8_t*, size_t, angzarr_buf*);

// The Go //export trampoline (defined in trampoline.go). Declared with
// non-const pointers because cgo //export cannot express const.
int32_t angzarrGoTrampoline(void*, uint64_t, uint8_t*, size_t,
                            uint8_t*, size_t, uint8_t*, size_t, angzarr_buf*);

// Shim with the exact angzarr_cb type; bridges the const-ness gap so the
// router stores one C function pointer that lands in the Go trampoline.
static int32_t angzarr_go_cb(void* ctx, uint64_t id,
        const uint8_t* tu, size_t tul, const uint8_t* p, size_t pl,
        const uint8_t* a, size_t al, angzarr_buf* out) {
    return angzarrGoTrampoline(ctx, id, (uint8_t*)tu, tul, (uint8_t*)p, pl,
                               (uint8_t*)a, al, out);
}

static int32_t angzarr_register(void* r, const uint8_t* d, size_t n) {
    return angzarr_router_register_aggregate(r, d, n, angzarr_go_cb);
}

static int32_t angzarr_register_projector(void* r, const uint8_t* d, size_t n) {
    return angzarr_router_register_projector(r, d, n, angzarr_go_cb);
}

static int32_t angzarr_register_saga(void* r, const uint8_t* d, size_t n) {
    return angzarr_router_register_saga(r, d, n, angzarr_go_cb);
}

static int32_t angzarr_register_process_manager(void* r, const uint8_t* d, size_t n) {
    return angzarr_router_register_process_manager(r, d, n, angzarr_go_cb);
}

// host_ctx is a runtime/cgo.Handle (an integer) reinterpreted as void*; the
// router treats it as opaque and hands it back to the trampoline. Casting
// through C keeps the Go side free of uintptr<->unsafe.Pointer churn.
static int32_t angzarr_dispatch_h(void* r, uintptr_t ctx,
        const uint8_t* req, size_t n, angzarr_buf* out) {
    return angzarr_router_dispatch(r, (void*)ctx, req, n, out);
}

static int32_t angzarr_dispatch_projector_h(void* r, uintptr_t ctx,
        const uint8_t* req, size_t n, angzarr_buf* out) {
    return angzarr_router_dispatch_projector(r, (void*)ctx, req, n, out);
}

static int32_t angzarr_dispatch_saga_h(void* r, uintptr_t ctx,
        const uint8_t* req, size_t n, angzarr_buf* out) {
    return angzarr_router_dispatch_saga(r, (void*)ctx, req, n, out);
}

static int32_t angzarr_dispatch_process_manager_h(void* r, uintptr_t ctx,
        const uint8_t* req, size_t n, angzarr_buf* out) {
    return angzarr_router_dispatch_process_manager(r, (void*)ctx, req, n, out);
}
*/
import "C"

import (
	"fmt"
	"runtime"
	"runtime/cgo"
	"sync"
	"unsafe"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/anypb"

	abipb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/router/ffi/v1"
	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// statusOKEmpty matches the ABI's STATUS_OK_EMPTY: success with no payload
// (a handler or compensator that emitted nothing).
const statusOKEmpty = 1

// AbiVersion reports the ABI version the linked router-ffi exposes.
// Bindings check it at load so a binding and a router-ffi artifact that
// have drifted refuse each other instead of marshaling garbage.
func AbiVersion() uint32 {
	return uint32(C.angzarr_abi_version())
}

// invoker is the type-erased bridge from a callback_id to a registered
// typed thunk. It receives the live dispatch session (holding the host
// state) and the marshaled callback inputs, and returns the response bytes
// plus a status code (0 ok+payload, 1 ok-empty, <0 coded error whose
// google.rpc.Status bytes are the returned payload).
type invoker func(s *session, typeURL string, payload, aux []byte) (out []byte, status int32)

// session is one dispatch's host-side state object, reached from callbacks
// via the host_ctx handle. State never crosses to Rust; it lives here and
// is created lazily by the first callback to run (all callbacks in one
// dispatch belong to the same aggregate, so the factory is consistent).
type session struct {
	router *Router
	state  any
}

// Router wraps the Rust core router plus the Go-side callback registry the
// trampoline routes through. Registration is not safe for concurrent use;
// concurrent Dispatch is — each dispatch parks its own state in a host_ctx
// the core isolates.
type Router struct {
	ptr      unsafe.Pointer
	mu       sync.Mutex
	registry map[uint64]invoker
	nextID   uint64
}

// NewRouter creates an empty router. Close it when done.
func NewRouter() *Router {
	return &Router{
		ptr:      C.angzarr_router_new(),
		registry: make(map[uint64]invoker),
	}
}

// Close frees the underlying Rust router. Safe to call once.
func (r *Router) Close() {
	if r.ptr != nil {
		C.angzarr_router_free(r.ptr)
		r.ptr = nil
	}
}

// assign records an invoker under a fresh callback id (caller holds r.mu).
func (r *Router) assign(inv invoker) uint64 {
	r.nextID++
	r.registry[r.nextID] = inv
	return r.nextID
}

// RegisterAggregate registers one aggregate component: it assigns callback
// ids to every thunk, serializes the AggregateDescriptor, and hands it to
// the core with the shared callback gateway. A free function (not a method)
// because Go methods cannot introduce the state type parameter.
func RegisterAggregate[S any](r *Router, d *AggregateDispatch[S]) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	factory := d.rebuilder.factory
	desc := &abipb.AggregateDescriptor{Name: d.name, Domain: d.domain}

	for fq, thunk := range d.rebuilder.appliers {
		id := r.assign(applierInvoker(factory, thunk))
		desc.Appliers = append(desc.Appliers, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if d.rebuilder.snapshot != nil {
		id := r.assign(applierInvoker(factory, d.rebuilder.snapshot))
		desc.SnapshotCallbackId = &id
	}
	for fq, thunk := range d.commands {
		id := r.assign(commandInvoker(factory, thunk))
		desc.Commands = append(desc.Commands, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	for fq, thunks := range d.rejections {
		entry := &abipb.RejectionEntry{FqCommandType: fq}
		for _, thunk := range thunks {
			id := r.assign(rejectionInvoker(factory, thunk))
			entry.CallbackIds = append(entry.CallbackIds, id)
		}
		desc.Rejections = append(desc.Rejections, entry)
	}

	descBytes, err := proto.Marshal(desc)
	if err != nil {
		return fmt.Errorf("marshal AggregateDescriptor: %w", err)
	}
	var dptr *C.uint8_t
	if len(descBytes) > 0 {
		dptr = (*C.uint8_t)(unsafe.Pointer(&descBytes[0]))
	}
	ret := C.angzarr_register(r.ptr, dptr, C.size_t(len(descBytes)))
	runtime.KeepAlive(descBytes)
	if ret != 0 {
		return decodeStatus(nil, int32(ret))
	}
	return nil
}

// RegisterProjector registers one projector component: it assigns callback
// ids to every fold/finish/unknown thunk, serializes the ProjectorDescriptor,
// and hands it to the core with the shared callback gateway. A free function
// (not a method) because Go methods cannot introduce the projection type
// parameter.
func RegisterProjector[P any](r *Router, d *ProjectorDispatch[P]) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	factory := d.factory
	desc := &abipb.ProjectorDescriptor{Name: d.name, Domains: d.domains}

	for fq, thunk := range d.events {
		id := r.assign(projectorEventInvoker(factory, thunk))
		desc.Events = append(desc.Events, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if d.unknown != nil {
		id := r.assign(projectorUnknownInvoker(d.unknown))
		desc.UnknownCallbackId = &id
	}
	if d.finish != nil {
		id := r.assign(projectorFinishInvoker(factory, d.finish))
		desc.FinishCallbackId = &id
	}

	descBytes, err := proto.Marshal(desc)
	if err != nil {
		return fmt.Errorf("marshal ProjectorDescriptor: %w", err)
	}
	var dptr *C.uint8_t
	if len(descBytes) > 0 {
		dptr = (*C.uint8_t)(unsafe.Pointer(&descBytes[0]))
	}
	ret := C.angzarr_register_projector(r.ptr, dptr, C.size_t(len(descBytes)))
	runtime.KeepAlive(descBytes)
	if ret != 0 {
		return decodeStatus(nil, int32(ret))
	}
	return nil
}

// RegisterSaga registers one saga component: it assigns callback ids to every
// event/rejection thunk, serializes the SagaDescriptor, and hands it to the
// core with the shared callback gateway. A method (not a free function) since
// a saga is stateless — it introduces no state type parameter.
func (r *Router) RegisterSaga(d *SagaDispatch) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	desc := &abipb.SagaDescriptor{
		Name:          d.name,
		InputDomain:   d.inputDomain,
		TargetDomains: d.targets,
	}
	for fq, thunk := range d.events {
		id := r.assign(sagaEventInvoker(thunk))
		desc.Events = append(desc.Events, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	for fq, thunks := range d.rejections {
		entry := &abipb.RejectionEntry{FqCommandType: fq}
		for _, thunk := range thunks {
			id := r.assign(sagaRejectionInvoker(thunk))
			entry.CallbackIds = append(entry.CallbackIds, id)
		}
		desc.Rejections = append(desc.Rejections, entry)
	}

	descBytes, err := proto.Marshal(desc)
	if err != nil {
		return fmt.Errorf("marshal SagaDescriptor: %w", err)
	}
	var dptr *C.uint8_t
	if len(descBytes) > 0 {
		dptr = (*C.uint8_t)(unsafe.Pointer(&descBytes[0]))
	}
	ret := C.angzarr_register_saga(r.ptr, dptr, C.size_t(len(descBytes)))
	runtime.KeepAlive(descBytes)
	if ret != 0 {
		return decodeStatus(nil, int32(ret))
	}
	return nil
}

// DispatchSaga runs one SagaHandleRequest through the registered saga and
// returns the SagaResponse, or a *CodedError decoded from the core's failure.
func (r *Router) DispatchSaga(req *pb.SagaHandleRequest) (*pb.SagaResponse, error) {
	reqBytes, err := proto.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("marshal SagaHandleRequest: %w", err)
	}

	h := cgo.NewHandle(&session{router: r})
	defer h.Delete()

	var reqPtr *C.uint8_t
	if len(reqBytes) > 0 {
		reqPtr = (*C.uint8_t)(unsafe.Pointer(&reqBytes[0]))
	}
	var out C.angzarr_buf
	ret := C.angzarr_dispatch_saga_h(r.ptr, C.uintptr_t(h), reqPtr, C.size_t(len(reqBytes)), &out)
	runtime.KeepAlive(reqBytes)
	respBytes := consumeBuf(&out)

	if ret == 0 {
		var resp pb.SagaResponse
		if err := proto.Unmarshal(respBytes, &resp); err != nil {
			return nil, fmt.Errorf("unmarshal SagaResponse: %w", err)
		}
		return &resp, nil
	}
	return nil, decodeStatus(respBytes, int32(ret))
}

// RegisterProcessManager registers one process-manager component: it assigns
// callback ids to every applier/snapshot/event/rejection thunk, serializes the
// ProcessManagerDescriptor, and hands it to the core with the shared callback
// gateway. A free function (not a method) because Go methods cannot introduce
// the state type parameter.
func RegisterProcessManager[S any](r *Router, d *ProcessManagerDispatch[S]) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	factory := d.rebuilder.factory
	desc := &abipb.ProcessManagerDescriptor{Name: d.name, PmDomain: d.pmDomain}

	for fq, thunk := range d.rebuilder.appliers {
		id := r.assign(applierInvoker(factory, thunk))
		desc.Appliers = append(desc.Appliers, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if d.rebuilder.snapshot != nil {
		id := r.assign(applierInvoker(factory, d.rebuilder.snapshot))
		desc.SnapshotCallbackId = &id
	}
	for inputDomain, byType := range d.handlers {
		for fq, thunk := range byType {
			id := r.assign(pmEventInvoker(factory, thunk))
			desc.Events = append(desc.Events, &abipb.PmEventEntry{
				InputDomain: inputDomain,
				FqType:      fq,
				CallbackId:  id,
			})
		}
	}
	for fq, thunks := range d.rejections {
		entry := &abipb.RejectionEntry{FqCommandType: fq}
		for _, thunk := range thunks {
			id := r.assign(pmRejectionInvoker(factory, thunk))
			entry.CallbackIds = append(entry.CallbackIds, id)
		}
		desc.Rejections = append(desc.Rejections, entry)
	}

	descBytes, err := proto.Marshal(desc)
	if err != nil {
		return fmt.Errorf("marshal ProcessManagerDescriptor: %w", err)
	}
	var dptr *C.uint8_t
	if len(descBytes) > 0 {
		dptr = (*C.uint8_t)(unsafe.Pointer(&descBytes[0]))
	}
	ret := C.angzarr_register_process_manager(r.ptr, dptr, C.size_t(len(descBytes)))
	runtime.KeepAlive(descBytes)
	if ret != 0 {
		return decodeStatus(nil, int32(ret))
	}
	return nil
}

// DispatchProcessManager runs one ProcessManagerHandleRequest through the
// registered PM and returns the ProcessManagerHandleResponse, or a *CodedError
// decoded from the core's failure.
func (r *Router) DispatchProcessManager(req *pb.ProcessManagerHandleRequest) (*pb.ProcessManagerHandleResponse, error) {
	reqBytes, err := proto.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("marshal ProcessManagerHandleRequest: %w", err)
	}

	h := cgo.NewHandle(&session{router: r})
	defer h.Delete()

	var reqPtr *C.uint8_t
	if len(reqBytes) > 0 {
		reqPtr = (*C.uint8_t)(unsafe.Pointer(&reqBytes[0]))
	}
	var out C.angzarr_buf
	ret := C.angzarr_dispatch_process_manager_h(r.ptr, C.uintptr_t(h), reqPtr, C.size_t(len(reqBytes)), &out)
	runtime.KeepAlive(reqBytes)
	respBytes := consumeBuf(&out)

	if ret == 0 {
		var resp pb.ProcessManagerHandleResponse
		if err := proto.Unmarshal(respBytes, &resp); err != nil {
			return nil, fmt.Errorf("unmarshal ProcessManagerHandleResponse: %w", err)
		}
		return &resp, nil
	}
	return nil, decodeStatus(respBytes, int32(ret))
}

// DispatchProjector folds one EventBook through the registered projector and
// returns the Projection, or a *CodedError decoded from the core's failure.
func (r *Router) DispatchProjector(book *pb.EventBook) (*pb.Projection, error) {
	reqBytes, err := proto.Marshal(book)
	if err != nil {
		return nil, fmt.Errorf("marshal EventBook: %w", err)
	}

	h := cgo.NewHandle(&session{router: r})
	defer h.Delete()

	var reqPtr *C.uint8_t
	if len(reqBytes) > 0 {
		reqPtr = (*C.uint8_t)(unsafe.Pointer(&reqBytes[0]))
	}
	var out C.angzarr_buf
	ret := C.angzarr_dispatch_projector_h(r.ptr, C.uintptr_t(h), reqPtr, C.size_t(len(reqBytes)), &out)
	runtime.KeepAlive(reqBytes)
	respBytes := consumeBuf(&out)

	if ret == 0 {
		var proj pb.Projection
		if err := proto.Unmarshal(respBytes, &proj); err != nil {
			return nil, fmt.Errorf("unmarshal Projection: %w", err)
		}
		return &proj, nil
	}
	return nil, decodeStatus(respBytes, int32(ret))
}

// Dispatch runs one ContextualCommand through the core and returns the
// BusinessResponse, or a *CodedError decoded from the core's failure.
func (r *Router) Dispatch(cc *pb.ContextualCommand) (*pb.BusinessResponse, error) {
	reqBytes, err := proto.Marshal(cc)
	if err != nil {
		return nil, fmt.Errorf("marshal ContextualCommand: %w", err)
	}

	// The session is reached from callbacks via this handle; the core holds
	// it only for the duration of this synchronous call.
	h := cgo.NewHandle(&session{router: r})
	defer h.Delete()

	var reqPtr *C.uint8_t
	if len(reqBytes) > 0 {
		reqPtr = (*C.uint8_t)(unsafe.Pointer(&reqBytes[0]))
	}
	var out C.angzarr_buf
	ret := C.angzarr_dispatch_h(r.ptr, C.uintptr_t(h), reqPtr, C.size_t(len(reqBytes)), &out)
	runtime.KeepAlive(reqBytes)
	respBytes := consumeBuf(&out)

	if ret == 0 {
		var resp pb.BusinessResponse
		if err := proto.Unmarshal(respBytes, &resp); err != nil {
			return nil, fmt.Errorf("unmarshal BusinessResponse: %w", err)
		}
		return &resp, nil
	}
	return nil, decodeStatus(respBytes, int32(ret))
}

// consumeBuf copies a router-allocated out buffer into Go memory and
// releases it (the dispatch out is router-owned).
func consumeBuf(b *C.angzarr_buf) []byte {
	if b.data == nil || b.len == 0 {
		return nil
	}
	out := C.GoBytes(unsafe.Pointer(b.data), C.int(b.len))
	C.angzarr_buf_release(b.data, b.len)
	b.data = nil
	b.len = 0
	return out
}

// applierInvoker / commandInvoker / rejectionInvoker build the type-erased
// bridge for one thunk, lazily seeding the session's state on first use.

func applierInvoker[S any](factory func() S, thunk ApplierThunk[S]) invoker {
	return func(s *session, typeURL string, payload, _ []byte) ([]byte, int32) {
		st := ensureState(s, factory)
		if err := thunk(st, &anypb.Any{TypeUrl: typeURL, Value: payload}); err != nil {
			return errorStatus(err)
		}
		return nil, 0
	}
}

func commandInvoker[S any](factory func() S, thunk CommandThunk[S]) invoker {
	return func(s *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var cax abipb.CommandContextAux
		if err := proto.Unmarshal(aux, &cax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal CommandContextAux: %w", err))
		}
		cctx := CommandContext{NextSequence: cax.NextSequence, HadPriorEvents: cax.HadPriorEvents}
		st := ensureState(s, factory)
		book, err := thunk(&anypb.Any{TypeUrl: typeURL, Value: payload}, st, cctx)
		if err != nil {
			return errorStatus(err)
		}
		if book == nil {
			return nil, statusOKEmpty
		}
		b, err := proto.Marshal(book)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal EventBook: %w", err))
		}
		return b, 0
	}
}

func rejectionInvoker[S any](factory func() S, thunk RejectionThunk[S]) invoker {
	return func(s *session, _ string, _, aux []byte) ([]byte, int32) {
		var rax abipb.RejectionAux
		if err := proto.Unmarshal(aux, &rax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal RejectionAux: %w", err))
		}
		var n pb.Notification
		if err := proto.Unmarshal(rax.Notification, &n); err != nil {
			return errorStatus(fmt.Errorf("unmarshal Notification: %w", err))
		}
		var rej pb.RejectionNotification
		if err := proto.Unmarshal(rax.Rejection, &rej); err != nil {
			return errorStatus(fmt.Errorf("unmarshal RejectionNotification: %w", err))
		}
		cctx := CommandContext{}
		if rax.Cctx != nil {
			cctx = CommandContext{NextSequence: rax.Cctx.NextSequence, HadPriorEvents: rax.Cctx.HadPriorEvents}
		}
		st := ensureState(s, factory)
		resp, err := thunk(&n, &rej, st, cctx)
		if err != nil {
			return errorStatus(err)
		}
		if resp == nil {
			return nil, statusOKEmpty
		}
		b, err := proto.Marshal(resp)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal BusinessResponse: %w", err))
		}
		return b, 0
	}
}

// projectorEventInvoker / projectorFinishInvoker / projectorUnknownInvoker
// build the type-erased bridge for the projector thunks.

func projectorEventInvoker[P any](factory func() P, thunk ProjectorEventThunk[P]) invoker {
	return func(s *session, typeURL string, payload, _ []byte) ([]byte, int32) {
		st := ensureState(s, factory)
		if err := thunk(st, &anypb.Any{TypeUrl: typeURL, Value: payload}); err != nil {
			return errorStatus(err)
		}
		return nil, 0
	}
}

func projectorFinishInvoker[P any](factory func() P, thunk ProjectorFinishThunk[P]) invoker {
	return func(s *session, _ string, payload, _ []byte) ([]byte, int32) {
		// The core hands the EventBook over as the callback payload so the
		// finisher can carry its cover onto the Projection.
		var book pb.EventBook
		if err := proto.Unmarshal(payload, &book); err != nil {
			return errorStatus(fmt.Errorf("unmarshal EventBook: %w", err))
		}
		st := ensureState(s, factory)
		proj, err := thunk(st, &book)
		if err != nil {
			return errorStatus(err)
		}
		b, err := proto.Marshal(proj)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal Projection: %w", err))
		}
		return b, 0
	}
}

func projectorUnknownInvoker(thunk ProjectorUnknownThunk) invoker {
	return func(_ *session, typeURL string, _, _ []byte) ([]byte, int32) {
		thunk(typeURL)
		return nil, 0
	}
}

// sagaEventInvoker / sagaRejectionInvoker bridge the saga thunks. A saga is
// stateless, so neither touches the session's host state — the event thunk
// rebuilds Destinations from the aux and returns a SagaResponse.

func sagaEventInvoker(thunk SagaEventThunk) invoker {
	return func(_ *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var sax abipb.SagaEventAux
		if err := proto.Unmarshal(aux, &sax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal SagaEventAux: %w", err))
		}
		dests := NewDestinations(sax.DestinationSequences)
		commands, events, err := thunk(&anypb.Any{TypeUrl: typeURL, Value: payload}, dests, sax.SourceCover)
		if err != nil {
			return errorStatus(err)
		}
		b, err := proto.Marshal(&pb.SagaResponse{Commands: commands, Events: events})
		if err != nil {
			return errorStatus(fmt.Errorf("marshal SagaResponse: %w", err))
		}
		return b, 0
	}
}

func sagaRejectionInvoker(thunk SagaRejectionThunk) invoker {
	return func(_ *session, _ string, _, aux []byte) ([]byte, int32) {
		var rax abipb.RejectionAux
		if err := proto.Unmarshal(aux, &rax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal RejectionAux: %w", err))
		}
		var n pb.Notification
		if err := proto.Unmarshal(rax.Notification, &n); err != nil {
			return errorStatus(fmt.Errorf("unmarshal Notification: %w", err))
		}
		var rej pb.RejectionNotification
		if err := proto.Unmarshal(rax.Rejection, &rej); err != nil {
			return errorStatus(fmt.Errorf("unmarshal RejectionNotification: %w", err))
		}
		events, err := thunk(&n, &rej)
		if err != nil {
			return errorStatus(err)
		}
		b, err := proto.Marshal(&pb.SagaResponse{Events: events})
		if err != nil {
			return errorStatus(fmt.Errorf("marshal SagaResponse: %w", err))
		}
		return b, 0
	}
}

// pmEventInvoker / pmRejectionInvoker bridge the process-manager thunks. The
// PM is stateful, so both lazily seed the session's state via the rebuilder
// factory (the appliers fold process_state into it first, exactly as the
// aggregate does).

func pmEventInvoker[S any](factory func() S, thunk PMEventThunk[S]) invoker {
	return func(s *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var pax abipb.PmEventAux
		if err := proto.Unmarshal(aux, &pax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal PmEventAux: %w", err))
		}
		dests := NewDestinations(pax.DestinationSequences)
		st := ensureState(s, factory)
		resp, err := thunk(&anypb.Any{TypeUrl: typeURL, Value: payload}, st, dests)
		if err != nil {
			return errorStatus(err)
		}
		b, err := proto.Marshal(resp)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal ProcessManagerHandleResponse: %w", err))
		}
		return b, 0
	}
}

func pmRejectionInvoker[S any](factory func() S, thunk PMRejectionThunk[S]) invoker {
	return func(s *session, _ string, _, aux []byte) ([]byte, int32) {
		var rax abipb.RejectionAux
		if err := proto.Unmarshal(aux, &rax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal RejectionAux: %w", err))
		}
		var n pb.Notification
		if err := proto.Unmarshal(rax.Notification, &n); err != nil {
			return errorStatus(fmt.Errorf("unmarshal Notification: %w", err))
		}
		var rej pb.RejectionNotification
		if err := proto.Unmarshal(rax.Rejection, &rej); err != nil {
			return errorStatus(fmt.Errorf("unmarshal RejectionNotification: %w", err))
		}
		st := ensureState(s, factory)
		processEvents, escalation, err := thunk(&n, &rej, st)
		if err != nil {
			return errorStatus(err)
		}
		resp := &pb.ProcessManagerHandleResponse{
			ProcessEvents: processEvents,
			Notification:  escalation,
		}
		b, err := proto.Marshal(resp)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal ProcessManagerHandleResponse: %w", err))
		}
		return b, 0
	}
}

// ensureState lazily creates the session's host state from the aggregate's
// factory on first callback, then reuses it across the dispatch.
func ensureState[S any](s *session, factory func() S) S {
	if s.state == nil {
		s.state = factory()
	}
	return s.state.(S)
}
