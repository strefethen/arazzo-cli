// Package runtime provides HTTP client, variable store, and workflow execution engine.
package runtime

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/antchfx/xmlquery"
	"github.com/tidwall/gjson"
	"golang.org/x/time/rate"
)

// Client is an HTTP client with rate limiting support.
type Client struct {
	httpClient *http.Client
	limiter    *rate.Limiter
	baseURLs   map[string]string // sourceDescription name -> base URL
	headers    map[string]string // default headers for all requests
}

// ClientOption configures a Client.
type ClientOption func(*Client)

// WithBaseURL sets a base URL for a source description.
func WithBaseURL(name, url string) ClientOption {
	return func(c *Client) {
		c.baseURLs[name] = url
	}
}

// WithRateLimit sets the rate limiter.
func WithRateLimit(rps float64, burst int) ClientOption {
	return func(c *Client) {
		c.limiter = rate.NewLimiter(rate.Limit(rps), burst)
	}
}

// WithTimeout sets the HTTP client timeout.
func WithTimeout(d time.Duration) ClientOption {
	return func(c *Client) {
		c.httpClient.Timeout = d
	}
}

// WithHeader sets a default header for all requests.
func WithHeader(key, value string) ClientOption {
	return func(c *Client) {
		c.headers[key] = value
	}
}

// NewClient creates a new Client with the given options.
func NewClient(opts ...ClientOption) *Client {
	c := &Client{
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
		limiter:  rate.NewLimiter(10, 20), // default: 10 req/s, burst 20
		baseURLs: make(map[string]string),
		headers:  make(map[string]string),
	}

	// Set default User-Agent
	c.headers["User-Agent"] = "arazzo-cli/0.1"

	for _, opt := range opts {
		opt(c)
	}

	return c
}

// RequestConfig describes an HTTP request to execute.
type RequestConfig struct {
	Method  string
	URL     string
	Headers map[string]string
	Body    []byte
	Timeout time.Duration
}

// Response wraps an HTTP response with helper methods for data extraction.
type Response struct {
	StatusCode  int
	Headers     http.Header
	Body        []byte
	ContentType string // "json" or "xml"
	xmlDoc      *xmlquery.Node
}

// Extract extracts a value from the response body using gjson (JSON) or XPath (XML).
func (r *Response) Extract(path string) any {
	if r.ContentType == "xml" {
		return r.ExtractXPath(path)
	}
	result := gjson.GetBytes(r.Body, path)
	if !result.Exists() {
		return nil
	}
	return result.Value()
}

// ExtractXPath extracts a value from XML using XPath.
func (r *Response) ExtractXPath(xpath string) any {
	if r.xmlDoc == nil {
		doc, err := xmlquery.Parse(bytes.NewReader(r.Body))
		if err != nil {
			return nil
		}
		r.xmlDoc = doc
	}

	node := xmlquery.FindOne(r.xmlDoc, xpath)
	if node == nil {
		return nil
	}
	return node.InnerText()
}

// ExtractString extracts a string value from the JSON body.
func (r *Response) ExtractString(path string) string {
	return gjson.GetBytes(r.Body, path).String()
}

// ExtractFloat extracts a float64 value from the JSON body.
func (r *Response) ExtractFloat(path string) float64 {
	return gjson.GetBytes(r.Body, path).Float()
}

// ExtractInt extracts an int64 value from the JSON body.
func (r *Response) ExtractInt(path string) int64 {
	return gjson.GetBytes(r.Body, path).Int()
}

// ExtractBool extracts a bool value from the JSON body.
func (r *Response) ExtractBool(path string) bool {
	return gjson.GetBytes(r.Body, path).Bool()
}

// ExtractArray extracts an array value from the JSON body.
func (r *Response) ExtractArray(path string) []gjson.Result {
	return gjson.GetBytes(r.Body, path).Array()
}

// Request executes an HTTP request and returns the response.
func (c *Client) Request(ctx context.Context, cfg RequestConfig) (*Response, error) {
	// Apply rate limiting
	if err := c.limiter.Wait(ctx); err != nil {
		return nil, fmt.Errorf("rate limiter: %w", err)
	}

	// Build request
	var body io.Reader
	if cfg.Body != nil {
		body = bytes.NewReader(cfg.Body)
	}

	req, err := http.NewRequestWithContext(ctx, cfg.Method, cfg.URL, body)
	if err != nil {
		return nil, fmt.Errorf("building request: %w", err)
	}

	// Apply default headers
	for k, v := range c.headers {
		req.Header.Set(k, v)
	}

	// Apply request-specific headers
	for k, v := range cfg.Headers {
		req.Header.Set(k, v)
	}

	// Execute request
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("executing request: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	// Read body
	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response body: %w", err)
	}

	// Detect content type
	contentType := "json" // default
	ct := resp.Header.Get("Content-Type")
	if strings.Contains(ct, "xml") || strings.Contains(ct, "rss") {
		contentType = "xml"
	}

	return &Response{
		StatusCode:  resp.StatusCode,
		Headers:     resp.Header,
		Body:        respBody,
		ContentType: contentType,
	}, nil
}

// GetBaseURL returns the base URL for a source description.
func (c *Client) GetBaseURL(name string) string {
	return c.baseURLs[name]
}
