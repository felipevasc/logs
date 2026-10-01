# Java trace structure

The Java parsers add five ordinary scalar fields: `java.exception.class`, `java.exception.message`, `java.root_cause.class`, `java.trace.complete`, and `java.trace.fingerprint`. The full frame structure is separate, on-demand detail metadata. It is not an ordinary `java.trace` Event field or a columnar value.

Existing raw text, message, exception text, flattened stacktrace and record boundaries remain unchanged. Only the `log4j` and `wildfly` engine variants receive the Java parser-enrichment revision; JSONL and standard Nginx/Apache variants retain their previous key calculation. Metadata journals and exclusion record-space identities are unchanged. Their in-memory Java column catalogs gain the supported scalar IDs, including when an older journal is reopened.

## Interpretation and limits

The parser uses a flat arena for root, cause and suppressed exceptions. Frames have parsed components or an explicit unknown state. Original spans are UTF-8 byte positions in the unchanged decoded raw block, not JavaScript string positions or original pre-decoding file offsets. Elided counts are retained without expanding frames.

Default bounds are 256 KiB of inspected trace input, 64 exception nodes, 1,024 frames, depth 32, 4,096 bytes per retained string and 256 KiB of structured credit. The credit conservatively covers retained structure and JSON escaping; it is not a bound on the entire existing Event or process memory. Explicit detail lookup additionally limits header inspection to 64 KiB. Cancellation in the cancellable parser is an error. The infallible source parser uses the bounded pure form and preserves its surrounding operation checks.

`complete` means the observed block was understood within these limits. It does not certify that the log producer printed every frame or that the original file was complete. Unknown lines, malformed child ownership, impossible elision counts, circular references and exhausted limits prevent a complete root-cause/fingerprint result. A root cause follows the observed main cause chain, excluding suppressed siblings. Missing historical raw remains unavailable; raw-backed historical details do not rewrite saved Events.

The version 1 fingerprint groups observed class/method/relation structure. It excludes messages, source filenames, line numbers and module versions, and is not an evidence identity. The `jvone` prefix plus Base32 digest is a single 57-character nonhex word. An actual Tantivy writer test verifies that it remains selective without creating a LONG marker or widening an existing 32-character hex-token domain.

The first grammar supports typical JDK trace rendering. Framework-specific suffixes such as Log4j extended packaging information are conservatively retained as unknown frame spans and make the interpretation incomplete.

## Why the rich tree is requested separately

Tiny synthetic fixtures compared the existing Java field extraction with an eager rich-tree field and scalar-only enrichment. These are serialized field bytes, not latency, whole-Event or process-memory measurements.

| Input frames | Existing fields | Eager rich-tree fields | Scalar-only fields |
| ---: | ---: | ---: | ---: |
| 10 | 596 B | 3,341 B | 860 B |
| 100 | 5,007 B | 26,633 B | 5,271 B |
| 1,000 | 50,908 B | 116,705 B | 51,033 B |

The 1,000-frame input reached structured credit at 308 retained frames and reported `payload_limit`; it did not silently claim a complete trace. The retained Trace structs and requested capacities were 4,043/27,981/106,589 bytes respectively, excluding allocator overhead, temporary parsing storage, serialized values, raw text and legacy fields. This supports keeping the verbose tree out of normal pages and prepared columnar data. A collapsed drawer alone would not prevent the eager allocation or wire cost.

## Format references

- [JDK Throwable.printStackTrace](https://docs.oracle.com/en/java/javase/25/docs/api/java.base/java/lang/Throwable.html#printStackTrace()): cause/suppressed structure, enclosing-frame elision and implementation-dependent text
- [JDK StackTraceElement.toString](https://docs.oracle.com/en/java/javase/25/docs/api/java.base/java/lang/StackTraceElement.html#toString()): loaders/modules and location forms
- [Log4j Pattern Layout](https://logging.apache.org/log4j/2.x/manual/pattern-layout.html): extended throwable packaging suffixes
