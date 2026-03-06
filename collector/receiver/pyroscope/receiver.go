package pyroscope

import (
	"bytes"
	"compress/gzip"
	"context"
	"fmt"
	"io"
	"mime/multipart"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"

	pprofprofile "github.com/google/pprof/profile"
	pproftranslator "github.com/open-telemetry/opentelemetry-collector-contrib/pkg/translator/pprof"
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/consumer/xconsumer"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/receiver"
	"go.uber.org/zap"
)

const ingestPath = "/ingest"

type pyroscopeReceiver struct {
	cfg        *Config
	settings   receiver.Settings
	logger     *zap.Logger
	next       xconsumer.Profiles
	httpServer *http.Server
	shutdownWg sync.WaitGroup
}

type ingestParams struct {
	name   string // app name (before the {labels} part)
	labels map[string]string
	start  uint64
	end    uint64
	format string // "pprof", "jfr", "folded"
}

func newPyroscopeReceiver(cfg *Config, consumer xconsumer.Profiles, set receiver.Settings) (*pyroscopeReceiver, error) {
	return &pyroscopeReceiver{
		cfg:      cfg,
		settings: set,
		logger:   set.Logger,
		next:     consumer,
	}, nil
}

func (r *pyroscopeReceiver) Start(ctx context.Context, host component.Host) error {
	mux := http.NewServeMux()
	mux.HandleFunc("POST "+ingestPath, r.handleIngest)

	var err error

	r.httpServer, err = r.cfg.HTTP.ToServer(ctx, host.GetExtensions(), r.settings.TelemetrySettings, mux)
	if err != nil {
		return fmt.Errorf("failed to create http server: %w", err)
	}

	var listener net.Listener

	listener, err = r.cfg.HTTP.ToListener(ctx)
	if err != nil {
		return fmt.Errorf("failed to create tcp listener: %w", err)
	}

	r.shutdownWg.Add(1)
	r.logger.Info("Starting Pyroscope receiver", zap.String("endpoint", r.cfg.HTTP.NetAddr.Endpoint))

	go func() {
		defer r.shutdownWg.Done()

		if err := r.httpServer.Serve(listener); err != nil && err != http.ErrServerClosed {
			r.logger.Error("HTTP server error", zap.Error(err))
		}
	}()

	return nil
}

func (r *pyroscopeReceiver) Shutdown(ctx context.Context) error {
	if r.httpServer != nil {
		if err := r.httpServer.Shutdown(ctx); err != nil {
			return fmt.Errorf("failed to shutdown http server: %w", err)
		}
	}

	r.shutdownWg.Wait()

	return nil
}

func (r *pyroscopeReceiver) handleIngest(resp http.ResponseWriter, req *http.Request) {
	ctx, cancel := context.WithTimeout(req.Context(), r.cfg.Timeout)
	defer cancel()

	params, err := parseIngestParams(req.URL.Query())
	if err != nil {
		http.Error(resp, err.Error(), http.StatusBadRequest)
		return
	}

	body, err := readBody(req)
	if err != nil {
		http.Error(resp, fmt.Sprintf("failed to read body: %s", err), http.StatusBadRequest)
		return
	}

	var profiles *pprofile.Profiles

	switch params.format {
	case "", "pprof":
		prof, pprofErr := pprofprofile.Parse(bytes.NewReader(body))
		if pprofErr != nil {
			// When format is unspecified, try folded stacks as fallback.
			// Pyroscope Java SDK sends collapsed stacks without setting format param.
			if params.format == "" {
				if foldedProf, foldedErr := parseFoldedStacks(body); foldedErr == nil {
					var convertErr error
					if profiles, convertErr = pproftranslator.ConvertPprofToProfiles(foldedProf); convertErr == nil {
						break
					}
				}
			}

			http.Error(resp, fmt.Sprintf("failed to parse pprof: %s", pprofErr), http.StatusBadRequest)

			return
		}

		var err error

		profiles, err = pproftranslator.ConvertPprofToProfiles(prof)
		if err != nil {
			http.Error(resp, fmt.Sprintf("failed to convert pprof to profiles: %s", err), http.StatusInternalServerError)
			return
		}

	case "folded", "collapsed":
		prof, err := parseFoldedStacks(body)
		if err != nil {
			http.Error(resp, fmt.Sprintf("failed to parse folded stacks: %s", err), http.StatusBadRequest)
			return
		}

		profiles, err = pproftranslator.ConvertPprofToProfiles(prof)
		if err != nil {
			http.Error(resp, fmt.Sprintf("failed to convert folded stacks to profiles: %s", err), http.StatusInternalServerError)
			return
		}

	default:
		http.Error(resp, fmt.Sprintf("unsupported format: %s (supported: pprof, folded)", params.format), http.StatusBadRequest)
		return
	}

	// Set resource attributes from ingest params.
	for i := range profiles.ResourceProfiles().Len() {
		rp := profiles.ResourceProfiles().At(i)
		attrs := rp.Resource().Attributes()
		attrs.PutStr("service.name", params.name)

		for k, v := range params.labels {
			attrs.PutStr(k, v)
		}
		// Set timestamps from query params if the profile doesn't have them.
		for j := range rp.ScopeProfiles().Len() {
			sp := rp.ScopeProfiles().At(j)
			for k := range sp.Profiles().Len() {
				p := sp.Profiles().At(k)
				if p.Time() == 0 && params.start > 0 {
					p.SetTime(pcommon.Timestamp(nsFromSec(params.start)))
				}

				if p.DurationNano() == 0 && params.end > params.start {
					p.SetDurationNano(nsFromSec(params.end) - nsFromSec(params.start))
				}
			}
		}
	}

	if err := r.next.ConsumeProfiles(ctx, *profiles); err != nil {
		http.Error(resp, err.Error(), http.StatusInternalServerError)
		return
	}

	r.logger.Debug("Ingested Pyroscope profile",
		zap.String("service", params.name),
		zap.String("format", params.format),
	)
	resp.WriteHeader(http.StatusOK)
}

func parseIngestParams(qs url.Values) (ingestParams, error) {
	name := qs.Get("name")
	if name == "" {
		return ingestParams{}, fmt.Errorf("required parameter 'name' is missing")
	}

	p := ingestParams{
		format: qs.Get("format"),
		labels: make(map[string]string),
	}

	// Parse name: "my.app.cpu{env=staging,region=us-west-1}" → name + labels.
	if idx := strings.Index(name, "{"); idx >= 0 {
		labelStr := strings.TrimSuffix(name[idx+1:], "}")
		p.name = name[:idx]

		if labelStr != "" {
			for _, pair := range strings.Split(labelStr, ",") {
				kv := strings.SplitN(pair, "=", 2)
				if len(kv) == 2 {
					p.labels[kv[0]] = kv[1]
				}
			}
		}
	} else {
		p.name = name
	}

	if v := qs.Get("from"); v != "" {
		start, err := strconv.ParseUint(v, 10, 64)
		if err != nil {
			return ingestParams{}, fmt.Errorf("failed to parse 'from': %w", err)
		}

		p.start = start
	}

	if v := qs.Get("until"); v != "" {
		end, err := strconv.ParseUint(v, 10, 64)
		if err != nil {
			return ingestParams{}, fmt.Errorf("failed to parse 'until': %w", err)
		}

		p.end = end
	}

	return p, nil
}

// readBody handles gzip-compressed and multipart/form-data bodies.
func readBody(req *http.Request) ([]byte, error) {
	ct := req.Header.Get("Content-Type")

	// Multipart form: Pyroscope SDKs send pprof as a form file.
	if strings.HasPrefix(ct, "multipart/form-data") {
		if err := req.ParseMultipartForm(32 << 20); err != nil {
			return nil, fmt.Errorf("failed to parse multipart form: %w", err)
		}

		defer func() { _ = req.MultipartForm.RemoveAll() }()

		for _, key := range []string{"profile", "pprof", "jfr"} {
			files, ok := req.MultipartForm.File[key]
			if !ok || len(files) == 0 {
				continue
			}

			f, err := files[0].Open()
			if err != nil {
				return nil, fmt.Errorf("failed to open form file: %w", err)
			}
			defer f.Close()

			return decompressReader(f)
		}

		return nil, fmt.Errorf("no profile/pprof/jfr field in multipart form")
	}

	// Regular body (possibly gzip-compressed).
	return decompressReader(req.Body)
}

func decompressReader(r io.Reader) ([]byte, error) {
	data, err := io.ReadAll(r)
	if err != nil {
		return nil, err
	}
	// Try gzip decompression; if it fails, assume raw.
	gz, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		return data, nil
	}
	defer gz.Close()

	decompressed, err := io.ReadAll(gz)
	if err != nil {
		// Not valid gzip — return raw.
		return data, nil
	}

	return decompressed, nil
}

// nsFromSec converts a timestamp that may be in seconds, milliseconds,
// microseconds, or nanoseconds to nanoseconds. Pyroscope SDKs send
// varying precisions. Thresholds chosen so that realistic timestamps
// (year 2000-2100) are classified correctly.
func nsFromSec(v uint64) uint64 {
	switch {
	case v < 1e12: // seconds (up to ~33658 AD)
		return v * 1e9
	case v < 1e15: // milliseconds
		return v * 1e6
	case v < 1e18: // microseconds
		return v * 1e3
	default: // already nanoseconds
		return v
	}
}

// parseFoldedStacks parses Brendan Gregg's folded/collapsed stacks format:
//
//	func_a;func_b;func_c 150
//	main;init;malloc 25
//
// Each line is a semicolon-separated stack trace followed by a space and a sample count.
// This is the format Pyroscope Java SDK sends after converting JFR locally.
func parseFoldedStacks(data []byte) (*pprofprofile.Profile, error) {
	lines := bytes.Split(data, []byte("\n"))

	funcMap := make(map[string]*pprofprofile.Function)
	locMap := make(map[string]*pprofprofile.Location)

	var nextFuncID, nextLocID uint64

	getFunc := func(name string) *pprofprofile.Function {
		if f, ok := funcMap[name]; ok {
			return f
		}

		nextFuncID++

		f := &pprofprofile.Function{ID: nextFuncID, Name: name}
		funcMap[name] = f

		return f
	}

	getLoc := func(name string) *pprofprofile.Location {
		if l, ok := locMap[name]; ok {
			return l
		}

		nextLocID++

		fn := getFunc(name)
		l := &pprofprofile.Location{
			ID:   nextLocID,
			Line: []pprofprofile.Line{{Function: fn}},
		}
		locMap[name] = l

		return l
	}

	var samples []*pprofprofile.Sample

	for _, line := range lines {
		line = bytes.TrimSpace(line)
		if len(line) == 0 {
			continue
		}

		// Find the last space — everything before is the stack, after is the count.
		lastSpace := bytes.LastIndexByte(line, ' ')
		if lastSpace < 0 {
			continue // Malformed line, skip.
		}

		stackStr := string(line[:lastSpace])
		countStr := string(bytes.TrimSpace(line[lastSpace+1:]))

		count, err := strconv.ParseInt(countStr, 10, 64)
		if err != nil {
			continue // Malformed count, skip.
		}

		frames := strings.Split(stackStr, ";")

		// Build locations in reverse order (leaf first, like pprof convention).
		locs := make([]*pprofprofile.Location, len(frames))
		for i, frame := range frames {
			locs[len(frames)-1-i] = getLoc(frame)
		}

		samples = append(samples, &pprofprofile.Sample{
			Location: locs,
			Value:    []int64{count},
		})
	}

	if len(samples) == 0 {
		return nil, fmt.Errorf("no valid stacks found in folded input")
	}

	// Collect functions and locations into slices.
	functions := make([]*pprofprofile.Function, 0, len(funcMap))
	for _, f := range funcMap {
		functions = append(functions, f)
	}

	locations := make([]*pprofprofile.Location, 0, len(locMap))
	for _, l := range locMap {
		locations = append(locations, l)
	}

	return &pprofprofile.Profile{
		SampleType: []*pprofprofile.ValueType{{Type: "samples", Unit: "count"}},
		PeriodType: &pprofprofile.ValueType{Type: "cpu", Unit: "nanoseconds"},
		Sample:     samples,
		Location:   locations,
		Function:   functions,
	}, nil
}

// Ensure multipart.File satisfies io.Reader at compile time.
var _ io.Reader = (multipart.File)(nil)
