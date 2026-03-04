# CSV Streaming Load Analysis

## Current Architecture

### Chart Streaming ("Add to Chart")
When a user clicks "Add to chart" on a signal, the app uses **incremental batch processing**:

1. **`populate_chart_data_for_signal`** adds the signal to `pending_signal_loads` with `start_idx: 0`
2. **`process_pending_signal_loads`** runs every frame (when Charts window is visible):
   - Processes up to **10,000 messages per frame** per signal
   - Decodes each message, extracts the signal value, adds point to chart
   - Advances `start_idx` until all messages are processed
   - Removes from `pending_signal_loads` when done

**Key insight:** The UI stays responsive because each frame does only a small chunk of work.

### Current CSV Loading (Blocking)
1. **`load_file`** spawns a background thread
2. Thread calls `load_file(&path)` which:
   - Reads entire file into memory (`std::fs::read`)
   - Parses CSV with `csv::Reader`—iterates all records in one go
   - Returns `Vec<CanMessage>` only when **fully complete**
3. Progress updates are sent **after** loading completes (bug: progress iterates over result, not during parse)
4. On `LoadingUpdate::Complete`, **`finish_loading`** runs on main thread:
   - Clones messages 4× (messages, playback, message_list, stats)
   - `populate_chart_data()` — iterates ALL messages if DBC loaded
   - `message_stats.update()` — processes all messages
   - `pattern_analyzer.analyze()` — processes all messages

**Freeze points:**
- CSV parsing blocks the background thread (fine; UI is responsive)
- **`finish_loading`** runs on main thread and can freeze for large files (150k+ messages)
- `populate_chart_data` does all-at-once when DBC is loaded

---

## Proposed: Stream CSV Loading

### Option A: Stream Parsing + Chunked Delivery
Parse CSV in batches in the background thread, send chunks to main thread:

```
Background thread:
  - Open file, read headers
  - Parse records in batches of 5,000–10,000
  - Send LoadingUpdate::Chunk(Vec<CanMessage>) for each batch
  - Send LoadingUpdate::Complete when done

Main thread:
  - Accumulate chunks into messages Vec
  - Update progress bar from chunk count
  - On Complete: do minimal finish_loading (no populate_chart_data)
  - Use existing pending_signal_loads for chart population
```

**Pros:** Progress reflects actual parsing; memory spikes less (chunks vs full).  
**Cons:** Need to change `LoadingUpdate` enum; `finish_loading` still does clones and stats.

### Option B: Stream Parsing + Defer All Heavy Work
Same as A, but also defer `message_stats.update` and `pattern_analyzer.analyze` to incremental processing (like `process_pending_signal_loads`).

**Pros:** Main thread stays responsive throughout.  
**Cons:** More refactoring; stats/analyzer need incremental APIs.

### Option C: Keep Full Load, Defer finish_loading Work
Keep current load (full parse in background), but when Complete arrives:
- Don't call `populate_chart_data` synchronously
- Add charted signals to `pending_signal_loads` (same as "Add to chart")
- Defer `message_stats.update` and `pattern_analyzer.analyze` to incremental batches

**Pros:** Simpler; reuses existing chart streaming.  
**Cons:** No progress during parse; memory still holds full file before UI updates.

---

## Recommended: Option A + Partial C

1. **Stream CSV parsing** — parse in batches, send `LoadingUpdate::Chunk` with progress
2. **Accumulate on main thread** — append chunks to `messages`; no full parse on main thread
3. **Keep `populate_chart_data` deferred** — only add to `pending_signal_loads` when DBC loaded (already done for "Add to chart")
4. **Defer stats/analyzer** — add `pending_stats_load` and `pending_analyzer_load` with same batch pattern, or run them in background after load

### Implementation Steps

1. **`src/input/csv.rs`**: Add `load_csv_streaming` that yields batches via callback or iterator
2. **`src/input/mod.rs`**: Add `load_file_streaming` that uses streaming CSV parser
3. **`src/main.rs`**: 
   - Add `LoadingUpdate::Chunk(Vec<CanMessage>, usize, usize)` for (batch, current, total)
   - Main thread accumulates chunks; on Complete, runs minimal `finish_loading`
   - Ensure `populate_chart_data` only adds to `pending_signal_loads` (no full iteration)
   - Defer or batch `message_stats.update` and `pattern_analyzer.analyze`

---

## Progress Estimation

For progress during CSV parse:
- **File size**: Use `metadata().len()` and `reader.position()` for approximate %
- **Line count**: Quick first pass: `wc -l` style or count lines while parsing (increment counter each batch)
