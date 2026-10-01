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
int32_t  angzarr_router_dispatch_fact(void*, void*, const uint8_t*, size_t, angzarr_buf*);
int32_t  angzarr_router_dispatch_replay(void*, void*, const uint8_t*, size_t, angzarr_buf*);

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

static int32_t angzarr_dispatch_fact_h(void* r, uintptr_t ctx,
        const uint8_t* req, size_t n, angzarr_buf* out) {
    return angzarr_router_dispatch_fact(r, (void*)ctx, req, n, out);
}

static int32_t angzarr_dispatch_replay_h(void* r, uintptr_t ctx,
        const uint8_t* req, size_t n, angzarr_buf* out) {
    return angzarr_router_dispatch_replay(r, (void*)ctx, req, n, out);
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
func AbiVersion() uint32 {
	return uint32(C.angzarr_abi_version())
}

// abiCheck verifies the linked library's ABI version once per process, so a
// binding and a router-ffi artifact that have drifted refuse each other
// instead of marshaling garbage.
var abiCheck = sync.OnceValue(func() error { return checkAbiVersion(AbiVersion()) })

// invoker is the type-erased bridge from a callback_id to a registered
// typed thunk. It receives the live dispatch session (holding the host
// state) and the marshaled callback inputs, and returns the response bytes
// plus a status code (0 ok+payload, 1 ok-empty, <0 coded error whose
// google.rpc.Status bytes are the returned payload).
type invoker func(s *session, typeURL string, payload, aux []byte) (out []byte, status int32)

// componentKey identifies one registered component within a Router. Every
// invoker registered for a component captures its key, so host state is
// looked up per component.
type componentKey uint64

// session is one dispatch's host-side state, reached from callbacks via the
// host_ctx handle. State never crosses to Rust; it lives here, keyed by
// component, and each component's state is created lazily by that
// component's first callback. One dispatch may run several components (e.g.
// co-resident process managers subscribed to the same domain), and each
// folds into and reads only its own state.
type session struct {
	router *Router
	states map[componentKey]any
}

// newSession starts an empty per-dispatch session for r.
func newSession(r *Router) *session {
	return &session{router: r, states: make(map[componentKey]any)}
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
	// nextComponent is the last component key handed out; each Register*
	// call takes a fresh one.
	nextComponent componentKey
}

// NewRouter creates an empty router. Close it when done. It panics if the
// linked router-ffi library's ABI version is not ExpectedAbiVersion.
func NewRouter() *Router {
	if err := abiCheck(); err != nil {
		panic(err)
	}
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

// newComponent returns a fresh component key (caller holds r.mu).
func (r *Router) newComponent() componentKey {
	r.nextComponent++
	return r.nextComponent
}

// assign records an invoker under a fresh callback id (caller holds r.mu).
func (r *Router) assign(inv invoker) uint64 {
	r.nextID++
	r.registry[r.nextID] = inv
	return r.nextID
}

// RegisterAggregate registers one aggregate component: it assigns callback
// ids to every thunk (plus a state packer for Replay when the state type is a
// protobuf message), serializes the AggregateDescriptor, and hands it to
// the core with the shared callback gateway. A free function (not a method)
// because Go methods cannot introduce the state type parameter.
func RegisterAggregate[S any](r *Router, d *AggregateDispatch[S]) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	key := r.newComponent()
	factory := d.rebuilder.factory
	desc := &abipb.AggregateDescriptor{Name: d.name, Domain: d.domain}

	for fq, thunk := range d.rebuilder.appliers {
		id := r.assign(applierContextInvoker(key, factory, thunk))
		desc.Appliers = append(desc.Appliers, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if d.rebuilder.snapshot != nil {
		id := r.assign(applierInvoker(key, factory, d.rebuilder.snapshot))
		desc.SnapshotCallbackId = &id
	}
	for fq, thunk := range d.commands {
		id := r.assign(commandInvoker(key, factory, thunk))
		desc.Commands = append(desc.Commands, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	for fq, thunks := range d.rejections {
		entry := &abipb.RejectionEntry{Compensates: fq}
		for _, thunk := range thunks {
			id := r.assign(rejectionInvoker(key, factory, thunk))
			entry.CallbackIds = append(entry.CallbackIds, id)
		}
		desc.Rejections = append(desc.Rejections, entry)
	}
	for fq, thunk := range d.undoes {
		id := r.assign(undoInvoker(key, factory, thunk))
		desc.Undoes = append(desc.Undoes, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	for fq, thunk := range d.facts {
		id := r.assign(factInvoker(key, factory, thunk))
		desc.Facts = append(desc.Facts, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if stateIsMessage[S]() {
		id := r.assign(statePackInvoker(key, factory))
		desc.StateCallbackId = &id
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

	key := r.newComponent()
	factory := d.factory
	desc := &abipb.ProjectorDescriptor{Name: d.name, Domains: d.domains}

	for fq, thunk := range d.events {
		id := r.assign(projectorEventInvoker(key, factory, thunk))
		desc.Events = append(desc.Events, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if d.unknown != nil {
		id := r.assign(projectorUnknownInvoker(d.unknown))
		desc.UnknownCallbackId = &id
	}
	if d.finish != nil {
		id := r.assign(projectorFinishInvoker(key, factory, d.finish))
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
// event thunk, serializes the SagaDescriptor, and hands it to the
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
	dests := NewDestinations(d.targets...)
	for fq, thunk := range d.events {
		id := r.assign(sagaEventInvoker(thunk, dests))
		desc.Events = append(desc.Events, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
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

	h := cgo.NewHandle(newSession(r))
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
// callback ids to every applier/snapshot/event/rejection thunk (plus a state
// packer for Replay when the state type is a protobuf message), serializes the
// ProcessManagerDescriptor, and hands it to the core with the shared callback
// gateway. A free function (not a method) because Go methods cannot introduce
// the state type parameter.
func RegisterProcessManager[S any](r *Router, d *ProcessManagerDispatch[S]) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	key := r.newComponent()
	factory := d.rebuilder.factory
	desc := &abipb.ProcessManagerDescriptor{Name: d.name, PmDomain: d.pmDomain, TargetDomains: d.targets}
	dests := NewDestinations(d.targets...)

	for fq, thunk := range d.rebuilder.appliers {
		id := r.assign(applierContextInvoker(key, factory, thunk))
		desc.Appliers = append(desc.Appliers, &abipb.CallbackEntry{FqType: fq, CallbackId: id})
	}
	if d.rebuilder.snapshot != nil {
		id := r.assign(applierInvoker(key, factory, d.rebuilder.snapshot))
		desc.SnapshotCallbackId = &id
	}
	for inputDomain, byType := range d.handlers {
		for fq, thunk := range byType {
			id := r.assign(pmEventInvoker(key, factory, thunk, dests))
			desc.Events = append(desc.Events, &abipb.PmEventEntry{
				InputDomain: inputDomain,
				FqType:      fq,
				CallbackId:  id,
			})
		}
	}
	for fq, thunks := range d.rejections {
		entry := &abipb.RejectionEntry{Compensates: fq}
		for _, thunk := range thunks {
			id := r.assign(pmRejectionInvoker(key, factory, thunk))
			entry.CallbackIds = append(entry.CallbackIds, id)
		}
		desc.Rejections = append(desc.Rejections, entry)
	}
	if stateIsMessage[S]() {
		id := r.assign(statePackInvoker(key, factory))
		desc.StateCallbackId = &id
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

	h := cgo.NewHandle(newSession(r))
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

	h := cgo.NewHandle(newSession(r))
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
	h := cgo.NewHandle(newSession(r))
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

// DispatchFact runs one FactRequest through the fact handling of the
// aggregate claiming the facts' cover domain and returns the EventBook of
// facts to record, or a *CodedError decoded from the core's failure.
func (r *Router) DispatchFact(req *pb.FactRequest) (*pb.EventBook, error) {
	reqBytes, err := proto.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("marshal FactRequest: %w", err)
	}

	h := cgo.NewHandle(newSession(r))
	defer h.Delete()

	var reqPtr *C.uint8_t
	if len(reqBytes) > 0 {
		reqPtr = (*C.uint8_t)(unsafe.Pointer(&reqBytes[0]))
	}
	var out C.angzarr_buf
	ret := C.angzarr_dispatch_fact_h(r.ptr, C.uintptr_t(h), reqPtr, C.size_t(len(reqBytes)), &out)
	runtime.KeepAlive(reqBytes)
	respBytes := consumeBuf(&out)

	if ret == 0 {
		var book pb.EventBook
		if err := proto.Unmarshal(respBytes, &book); err != nil {
			return nil, fmt.Errorf("unmarshal EventBook: %w", err)
		}
		return &book, nil
	}
	return nil, decodeStatus(respBytes, int32(ret))
}

// DispatchReplay rebuilds the state of the aggregate registered for domain
// (empty selects a sole registered aggregate), or of the process manager
// whose own domain it is when no aggregate claims it, from req and returns it
// packed in a ReplayResponse, or a *CodedError decoded from the core's
// failure. A component whose state is not a protobuf message does not
// support Replay (NO_HANDLER_REGISTERED).
func (r *Router) DispatchReplay(domain string, req *pb.ReplayRequest) (*pb.ReplayResponse, error) {
	reqBytes, err := proto.Marshal(&abipb.ReplayCall{Domain: domain, Request: req})
	if err != nil {
		return nil, fmt.Errorf("marshal ReplayCall: %w", err)
	}

	h := cgo.NewHandle(newSession(r))
	defer h.Delete()

	var reqPtr *C.uint8_t
	if len(reqBytes) > 0 {
		reqPtr = (*C.uint8_t)(unsafe.Pointer(&reqBytes[0]))
	}
	var out C.angzarr_buf
	ret := C.angzarr_dispatch_replay_h(r.ptr, C.uintptr_t(h), reqPtr, C.size_t(len(reqBytes)), &out)
	runtime.KeepAlive(reqBytes)
	respBytes := consumeBuf(&out)

	if ret == 0 {
		var resp pb.ReplayResponse
		if err := proto.Unmarshal(respBytes, &resp); err != nil {
			return nil, fmt.Errorf("unmarshal ReplayResponse: %w", err)
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

// applierContextInvoker decodes the applier's ProjectorEventAux into the
// PageContext the thunk receives.
func applierContextInvoker[S any](key componentKey, factory func() S, thunk ApplierContextThunk[S]) invoker {
	return func(s *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var pax abipb.ProjectorEventAux
		if err := proto.Unmarshal(aux, &pax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal ProjectorEventAux: %w", err))
		}
		ctx := PageContext{Cover: pax.Cover, Sequence: pax.Sequence}
		st := ensureState(s, key, factory)
		if err := thunk(st, &anypb.Any{TypeUrl: typeURL, Value: payload}, ctx); err != nil {
			return errorStatus(err)
		}
		return nil, 0
	}
}

// applierInvoker / commandInvoker / rejectionInvoker / undoInvoker /
// factInvoker build the type-erased bridge for one thunk, lazily seeding the
// component's state on first use.

func applierInvoker[S any](key componentKey, factory func() S, thunk ApplierThunk[S]) invoker {
	return func(s *session, typeURL string, payload, _ []byte) ([]byte, int32) {
		st := ensureState(s, key, factory)
		if err := thunk(st, &anypb.Any{TypeUrl: typeURL, Value: payload}); err != nil {
			return errorStatus(err)
		}
		return nil, 0
	}
}

func commandInvoker[S any](key componentKey, factory func() S, thunk CommandThunk[S]) invoker {
	return func(s *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var cax abipb.CommandContextAux
		if err := proto.Unmarshal(aux, &cax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal CommandContextAux: %w", err))
		}
		cctx := commandContextFrom(&cax)
		st := ensureState(s, key, factory)
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

func rejectionInvoker[S any](key componentKey, factory func() S, thunk RejectionThunk[S]) invoker {
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
		st := ensureState(s, key, factory)
		resp, err := thunk(&n, &rej, st, commandContextFrom(rax.Cctx))
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

func undoInvoker[S any](key componentKey, factory func() S, thunk UndoThunk[S]) invoker {
	return func(s *session, _ string, _, aux []byte) ([]byte, int32) {
		var uax abipb.UndoAux
		if err := proto.Unmarshal(aux, &uax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal UndoAux: %w", err))
		}
		var n pb.Notification
		if err := proto.Unmarshal(uax.Notification, &n); err != nil {
			return errorStatus(fmt.Errorf("unmarshal Notification: %w", err))
		}
		var compensate pb.Compensate
		if err := proto.Unmarshal(uax.Compensate, &compensate); err != nil {
			return errorStatus(fmt.Errorf("unmarshal Compensate: %w", err))
		}
		st := ensureState(s, key, factory)
		resp, err := thunk(&n, &compensate, st, commandContextFrom(uax.Cctx))
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

func factInvoker[S any](key componentKey, factory func() S, thunk FactThunk[S]) invoker {
	return func(s *session, typeURL string, payload, _ []byte) ([]byte, int32) {
		st := ensureState(s, key, factory)
		record, err := thunk(&anypb.Any{TypeUrl: typeURL, Value: payload}, st)
		if err != nil {
			return errorStatus(err)
		}
		b, err := proto.Marshal(&abipb.FactRecord{Fact: record.Fact, Flags: record.Flags})
		if err != nil {
			return errorStatus(fmt.Errorf("marshal FactRecord: %w", err))
		}
		return b, 0
	}
}

// statePackInvoker packs the component's current session state as a
// serialized google.protobuf.Any (bare "/" type-URL prefix) for Replay.
func statePackInvoker[S any](key componentKey, factory func() S) invoker {
	return func(s *session, _ string, _, _ []byte) ([]byte, int32) {
		st := ensureState(s, key, factory)
		msg, ok := any(st).(proto.Message)
		if !ok {
			return errorStatus(fmt.Errorf("component state %T is not a protobuf message", st))
		}
		packed, err := Pack(msg)
		if err != nil {
			return errorStatus(fmt.Errorf("pack state: %w", err))
		}
		b, err := proto.Marshal(packed)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal state Any: %w", err))
		}
		return b, 0
	}
}

// stateIsMessage reports whether the state type S is a protobuf message (and
// so can be packed for Replay).
func stateIsMessage[S any]() bool {
	var zero S
	_, ok := any(zero).(proto.Message)
	return ok
}

// commandContextFrom builds the CommandContext a handler sees from the core's
// CommandContextAux (nil yields the zero context).
func commandContextFrom(aux *abipb.CommandContextAux) CommandContext {
	return CommandContext{
		NextSequence:   aux.GetNextSequence(),
		HadPriorEvents: aux.GetHadPriorEvents(),
		Cover:          aux.GetCover(),
	}
}

// projectorEventInvoker / projectorFinishInvoker / projectorUnknownInvoker
// build the type-erased bridge for the projector thunks.

func projectorEventInvoker[P any](key componentKey, factory func() P, thunk ProjectorEventContextThunk[P]) invoker {
	return func(s *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var pax abipb.ProjectorEventAux
		if err := proto.Unmarshal(aux, &pax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal ProjectorEventAux: %w", err))
		}
		ctx := PageContext{Cover: pax.Cover, Sequence: pax.Sequence}
		st := ensureState(s, key, factory)
		if err := thunk(st, &anypb.Any{TypeUrl: typeURL, Value: payload}, ctx); err != nil {
			return errorStatus(err)
		}
		return nil, 0
	}
}

func projectorFinishInvoker[P any](key componentKey, factory func() P, thunk ProjectorFinishThunk[P]) invoker {
	return func(s *session, _ string, payload, _ []byte) ([]byte, int32) {
		// The core hands the EventBook over as the callback payload so the
		// finisher can carry its cover onto the Projection.
		var book pb.EventBook
		if err := proto.Unmarshal(payload, &book); err != nil {
			return errorStatus(fmt.Errorf("unmarshal EventBook: %w", err))
		}
		st := ensureState(s, key, factory)
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

// sagaEventInvoker bridges a saga event thunk. A saga is stateless, so it
// never touches the session's host state; it hands the thunk the saga's
// declared Destinations and the source event's PageContext, and returns a
// SagaResponse.
func sagaEventInvoker(thunk SagaEventContextThunk, dests *Destinations) invoker {
	return func(_ *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var sax abipb.SagaEventAux
		if err := proto.Unmarshal(aux, &sax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal SagaEventAux: %w", err))
		}
		source := PageContext{Cover: sax.SourceCover, Sequence: sax.SourceSeq}
		commands, events, err := thunk(&anypb.Any{TypeUrl: typeURL, Value: payload}, dests, source)
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

// pmEventInvoker / pmRejectionInvoker bridge the process-manager thunks. The
// PM is stateful, so both lazily seed the PM's own state via the rebuilder
// factory (the appliers fold process_state into it first, exactly as the
// aggregate does).

func pmEventInvoker[S any](key componentKey, factory func() S, thunk PMEventCoverThunk[S], dests *Destinations) invoker {
	return func(s *session, typeURL string, payload, aux []byte) ([]byte, int32) {
		var pax abipb.PmEventAux
		if err := proto.Unmarshal(aux, &pax); err != nil {
			return errorStatus(fmt.Errorf("unmarshal PmEventAux: %w", err))
		}
		st := ensureState(s, key, factory)
		resp, err := thunk(&anypb.Any{TypeUrl: typeURL, Value: payload}, st, dests, pax.TriggerCover)
		if err != nil {
			return errorStatus(err)
		}
		if resp == nil {
			return nil, statusOKEmpty
		}
		b, err := proto.Marshal(resp)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal ProcessManagerHandleResponse: %w", err))
		}
		return b, 0
	}
}

func pmRejectionInvoker[S any](key componentKey, factory func() S, thunk PMCompensatorThunk[S]) invoker {
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
		st := ensureState(s, key, factory)
		resp, err := thunk(&n, &rej, st)
		if err != nil {
			return errorStatus(err)
		}
		if resp == nil {
			return nil, statusOKEmpty
		}
		b, err := proto.Marshal(resp)
		if err != nil {
			return errorStatus(fmt.Errorf("marshal ProcessManagerHandleResponse: %w", err))
		}
		return b, 0
	}
}

// ensureState returns the host state of the component identified by key,
// creating it from that component's factory on its first callback in this
// dispatch and reusing it for the component's later callbacks.
func ensureState[S any](s *session, key componentKey, factory func() S) S {
	st, ok := s.states[key]
	if !ok {
		st = factory()
		s.states[key] = st
	}
	return st.(S)
}
