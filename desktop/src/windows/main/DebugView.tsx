import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, Braces, Check, Clipboard, Pause, Play, RotateCcw, TerminalSquare } from "lucide-react";
import {
  clearDebugCapture,
  getDebugCapture,
  setDebugCapture,
  type DebugEvent,
} from "@/bridge";
import type { StatusPicture } from "@/bridge";
import { cn } from "@/lib/utils";

function number(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function string(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function object(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? Object.fromEntries(Object.entries(value))
    : null;
}

function eventLabel(event: DebugEvent): string {
  switch (event.kind) {
    case "request": return `API request · ${string(event.data.stage) ?? "model"}`;
    case "model_send": return `Sent to ${string(event.data.model) ?? "model"}`;
    case "transport_attempt": return `${string(event.data.mode) === "stream" ? "Streaming" : "Fallback"} API attempt`;
    case "first_delta": return "First response bytes";
    case "response": return "API response";
    case "request_error": return "API request failed";
    case "tool_start": return `${string(event.data.tool_name) ?? "Tool"} started`;
    case "history": return "Complete chat history";
    case "tool_end": return `${string(event.data.tool_name) ?? "Tool"} finished`;
    case "round_start": return `Agent round ${number(event.data.round) ?? ""}`;
    case "round_end": return `Round ${number(event.data.round) ?? ""} ended`;
    case "message": return `${string(event.data.role) ?? "Chat"} message`;
    case "confirmation": return "Approval requested";
    case "input": return "Input requested";
    case "agent_start": return "Agent started";
    case "agent_end": return "Agent finished";
    case "error": return "Agent error";
    case "tool_note": return "Tool note";
    default: return event.kind.replace(/_/g, " ");
  }
}

function eventDetail(event: DebugEvent): string {
  if (event.kind === "request") {
    const messages = event.data.messages;
    return `${Array.isArray(messages) ? messages.length : 0} messages · ${number(event.data.estimate_tokens)?.toLocaleString() ?? "?"} est. tokens`;
  }
  if (event.kind === "history") return `${Array.isArray(event.data.messages) ? event.data.messages.length : 0} transcript messages`;
  if (event.kind === "response") {
    const usage = object(event.data.usage);
    return `${number(event.data.duration_ms)?.toLocaleString() ?? "?"} ms${usage ? ` · ${number(usage.prompt_tokens)?.toLocaleString() ?? "?"} input tokens` : " · usage unavailable"}`;
  }
  if (event.kind === "tool_start") return string(event.data.args_summary) ?? "";
  if (event.kind === "tool_end") return `${number(event.data.duration_ms)?.toLocaleString() ?? "?"} ms · ${event.data.ok === true ? "success" : "failed"}`;
  if (event.kind === "model_send" || event.kind === "transport_attempt") return `Request #${number(event.data.request_seq) ?? "?"}`;
  if (event.kind === "request_error" || event.kind === "error") return string(event.data.message) ?? "";
  if (event.kind === "first_delta") return `${number(event.data.ttfb_ms)?.toLocaleString() ?? "?"} ms to first byte`;
  if (event.kind === "message") return string(event.data.preview) ?? "";
  return "";
}

function formatTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString([], { hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function JsonBlock({ value }: { value: unknown }) {
  return <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-xl border border-white/[0.055] bg-black/25 p-3 font-mono text-[11px] leading-[1.55] text-white/65">{JSON.stringify(value, null, 2)}</pre>;
}

export function DebugView({ status }: { status: StatusPicture }) {
  const [events, setEvents] = useState<DebugEvent[]>([]);
  const [enabled, setEnabled] = useState(false);
  const [selectedSeq, setSelectedSeq] = useState<number | null>(null);
  const [droppedEvents, setDroppedEvents] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const cursor = useRef(0);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const snapshot = await getDebugCapture(cursor.current);
      setEnabled(snapshot.enabled);
      setDroppedEvents(snapshot.dropped_events);
      cursor.current = snapshot.latest_seq;
      if (snapshot.events.length) {
        setEvents((previous) => {
          const retained = previous.filter((item) => item.seq >= snapshot.oldest_seq);
          return [...retained, ...snapshot.events];
        });
      } else {
        setEvents((previous) => previous.filter((item) => item.seq >= snapshot.oldest_seq));
      }
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    void refresh();
    const interval = window.setInterval(() => void refresh(), 500);
    return () => window.clearInterval(interval);
  }, [refresh]);

  const selected = events.find((event) => event.seq === selectedSeq) ?? events[events.length - 1] ?? null;
  const requests = events.filter((event) => event.kind === "request");
  const toolStarts = events.filter((event) => event.kind === "tool_start");
  const latestRequest = requests[requests.length - 1] ?? null;
  const latestResponse = latestRequest
    ? [...events].reverse().find((event) => event.kind === "response" && number(event.data.request_seq) === latestRequest.seq)
    : null;
  const latestUsage = object(latestResponse?.data.usage);
  const contextUsed = number(latestUsage?.prompt_tokens) ?? number(latestRequest?.data.estimate_tokens);
  const contextLimit = number(latestRequest?.data.context_limit);
  const contextPct = contextUsed !== null && contextLimit ? Math.min(100, Math.round(contextUsed / contextLimit * 100)) : null;
  const selectedRequest = selected?.kind === "request" ? selected : null;
  const selectedResponse = selectedRequest
    ? [...events].reverse().find((event) => event.kind === "response" && number(event.data.request_seq) === selectedRequest.seq)
    : null;
  const messageRows: unknown[] = Array.isArray(selectedRequest?.data.messages) ? selectedRequest.data.messages : [];
  const breakdown = useMemo(() => Object.entries(object(selectedRequest?.data.breakdown) ?? {})
    .map(([name, value]) => ({ name, tokens: number(value) ?? 0 }))
    .sort((a, b) => b.tokens - a.tokens), [selectedRequest]);
  const breakdownTotal = breakdown.reduce((sum, item) => sum + item.tokens, 0);

  const toggle = async () => {
    try {
      await setDebugCapture(!enabled);
      setEnabled(!enabled);
      setError(null);
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const clear = async () => {
    try {
      await clearDebugCapture();
      setEvents([]);
      setSelectedSeq(null);
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(events, null, 2));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  return (
    <div className="mx-auto w-full max-w-6xl px-5 pb-10 pt-6 sm:px-7">
      <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="mb-1 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.16em] text-[#73adff]">
            <Activity className="size-3.5" /> Developer monitor
          </p>
          <h1 className="text-[26px] font-semibold tracking-[-0.045em] text-white">Inside a Boris turn</h1>
          <p className="mt-1 max-w-xl text-[12px] leading-relaxed text-white/42">Inspect model requests, chat context, token use, and tool execution as they happen.</p>
        </div>
        <div className="flex items-center gap-2">
          <button type="button" onClick={() => void toggle()} className={cn("inline-flex h-9 items-center gap-2 rounded-[10px] border px-3 text-[12px] font-medium transition-colors", enabled ? "border-[#72adff]/30 bg-[#3288ff]/15 text-[#a9cdff] hover:bg-[#3288ff]/25" : "border-white/10 bg-white/[0.055] text-white/70 hover:bg-white/[0.09]")}>{enabled ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}{enabled ? "Pause capture" : "Start capture"}</button>
          <button type="button" title="Copy captured events as JSON" aria-label="Copy captured events as JSON" disabled={!events.length} onClick={() => void copy()} className="flex size-9 items-center justify-center rounded-[10px] border border-white/10 bg-white/[0.055] text-white/60 transition-colors hover:bg-white/[0.09] disabled:opacity-35">{copied ? <Check className="size-4" /> : <Clipboard className="size-4" />}</button>
          <button type="button" title="Clear captured events" aria-label="Clear captured events" disabled={!events.length} onClick={() => void clear()} className="flex size-9 items-center justify-center rounded-[10px] border border-white/10 bg-white/[0.055] text-white/60 transition-colors hover:bg-white/[0.09] disabled:opacity-35"><RotateCcw className="size-4" /></button>
        </div>
      </div>

      <div className="mb-5 grid grid-cols-2 gap-2.5 sm:grid-cols-4">
        <Metric label="Capture" value={enabled ? "Live" : "Paused"} detail={status.phase === "Thinking" ? "Boris is working" : `Engine · ${status.phase}`} accent={enabled} />
        <Metric label="API trips" value={String(events.filter((event) => event.kind === "transport_attempt").length)} detail={`${requests.length} agent requests`} />
        <Metric label="Tool runs" value={String(toolStarts.length)} detail={`${events.filter((event) => event.kind === "tool_end").length} completed`} />
        <Metric label="Context" value={contextPct === null ? "—" : `${contextPct}%`} detail={contextUsed === null ? "Waiting for a request" : `${contextUsed.toLocaleString()} / ${contextLimit?.toLocaleString() ?? "?"} tokens${latestUsage ? " · provider" : " · estimate"}`} />
      </div>

      <p className="mb-4 rounded-xl border border-white/[0.06] bg-white/[0.025] px-3.5 py-2.5 text-[11px] leading-relaxed text-white/42">Capture begins when you turn it on and stays in memory only. Requests may contain personal chat, tool output, and secrets you typed into Boris. Copy JSON only when you intend to share it.</p>
      {droppedEvents > 0 && <p className="mb-3 text-[11px] text-[#ffc47d]">{droppedEvents} older events were dropped from the memory buffer.</p>}
      {error && <p className="mb-3 rounded-lg bg-[#ff3b30]/10 px-3 py-2 text-[12px] text-[#ff9a93]">{error}</p>}

      <div className="grid min-h-[28rem] gap-3 md:grid-cols-[minmax(15rem,0.9fr)_minmax(0,1.6fr)]">
        <section className="overflow-hidden rounded-[17px] border border-white/[0.07] bg-white/[0.035]" aria-label="Captured timeline">
          <div className="flex items-center justify-between border-b border-white/[0.065] px-4 py-3.5"><h2 className="text-[12px] font-semibold text-white/85">Timeline</h2><span className="font-mono text-[10px] text-white/35">{events.length} events</span></div>
          <div className="max-h-[39rem] overflow-y-auto p-1.5">
            {!events.length ? <div className="flex min-h-56 flex-col items-center justify-center px-6 text-center"><Activity className="mb-3 size-5 text-white/25" /><p className="text-[13px] text-white/60">No captured activity yet</p><p className="mt-1 text-[11px] leading-relaxed text-white/35">Start capture, then ask Boris to do something.</p></div> : [...events].reverse().map((event) => <button key={event.seq} type="button" onClick={() => setSelectedSeq(event.seq)} className={cn("mb-0.5 flex w-full gap-3 rounded-xl px-3 py-2.5 text-left transition-colors", selected?.seq === event.seq ? "bg-[#3288ff]/[0.13]" : "hover:bg-white/[0.045]")}>
              <span className={cn("mt-1.5 size-1.5 shrink-0 rounded-full", event.kind === "request" ? "bg-[#72adff]" : event.kind === "response" ? "bg-[#6bd0ad]" : event.kind.includes("error") ? "bg-[#ff8e86]" : event.kind.startsWith("tool") ? "bg-[#ffc47d]" : "bg-white/30")} />
              <span className="min-w-0 flex-1"><span className="flex items-center justify-between gap-2"><span className="truncate text-[12px] font-medium text-white/78">{eventLabel(event)}</span><span className="shrink-0 font-mono text-[10px] text-white/30">{formatTime(event.at_ms)}</span></span><span className="mt-0.5 block truncate text-[11px] text-white/38">{eventDetail(event) || event.turn_id || `Event ${event.seq}`}</span></span>
            </button>)}
          </div>
        </section>

        <section className="min-w-0 overflow-hidden rounded-[17px] border border-white/[0.07] bg-white/[0.035]" aria-label="Event details">
          <div className="flex items-center justify-between border-b border-white/[0.065] px-4 py-3.5"><h2 className="text-[12px] font-semibold text-white/85">{selected ? eventLabel(selected) : "Details"}</h2><span className="font-mono text-[10px] text-white/35">{selected ? `#${selected.seq}` : ""}</span></div>
          {!selected ? <div className="flex min-h-56 items-center justify-center text-[12px] text-white/35">Select an event to inspect it.</div> : <div className="space-y-5 p-4 sm:p-5">
            <div className="flex flex-wrap gap-x-5 gap-y-1 font-mono text-[10px] text-white/35"><span>{new Date(selected.at_ms).toLocaleString()}</span>{selected.turn_id && <span>Turn {selected.turn_id}</span>}</div>
            {selectedRequest && <>
              <div className="grid gap-2 sm:grid-cols-3"><SmallStat label="Input estimate" value={`${(number(selectedRequest.data.estimate_tokens) ?? 0).toLocaleString()} tokens`} /><SmallStat label="Model window" value={`${(number(selectedRequest.data.context_limit) ?? 0).toLocaleString()} tokens`} /><SmallStat label="Output reserve" value={`${(number(selectedRequest.data.output_reserve) ?? 0).toLocaleString()} tokens`} /></div>
              <div><div className="mb-2 flex items-center justify-between"><h3 className="text-[12px] font-semibold text-white/78">What filled the request</h3><span className="text-[10px] text-white/32">local estimates</span></div><div className="space-y-2">{breakdown.map((item) => <div key={item.name} className="grid grid-cols-[8rem_minmax(0,1fr)_4.5rem] items-center gap-2 text-[10px]"><span className="truncate text-white/48">{item.name}</span><div className="h-1.5 overflow-hidden rounded-full bg-white/[0.07]"><div className="h-full rounded-full bg-[#6daaff]" style={{ width: `${breakdownTotal ? item.tokens / breakdownTotal * 100 : 0}%` }} /></div><span className="text-right font-mono text-white/50">{breakdownTotal ? Math.round(item.tokens / breakdownTotal * 100) : 0}%</span></div>)}</div></div>
              {selectedResponse && <div className="rounded-xl border border-[#6bd0ad]/[0.16] bg-[#6bd0ad]/[0.045] p-3"><p className="text-[11px] font-medium text-[#a2ebce]">Provider response</p><p className="mt-1 text-[11px] text-white/50">{number(selectedResponse.data.duration_ms)?.toLocaleString() ?? "?"} ms · {number(object(selectedResponse.data.usage)?.prompt_tokens)?.toLocaleString() ?? "Usage unavailable"} input tokens · {number(object(selectedResponse.data.usage)?.completion_tokens)?.toLocaleString() ?? "?"} output tokens</p></div>}
              <div><h3 className="mb-2 text-[12px] font-semibold text-white/78">Messages sent to the model</h3><MessageList rows={messageRows} /></div>
              <details className="group rounded-xl border border-white/[0.06] bg-black/[0.12]"><summary className="cursor-pointer px-3 py-2.5 text-[11px] text-white/68">Tool schemas sent ({Array.isArray(selectedRequest.data.tools) ? selectedRequest.data.tools.length : 0})</summary><div className="px-2 pb-2"><JsonBlock value={selectedRequest.data.tools} /></div></details>
            </>}
            {selected.kind === "history" && <div><h3 className="mb-2 text-[12px] font-semibold text-white/78">Complete session transcript</h3><p className="mb-3 text-[11px] leading-relaxed text-white/38">This includes older messages that compaction removed from the model request.</p><MessageList rows={Array.isArray(selected.data.messages) ? selected.data.messages : []} /></div>}
            {!selectedRequest && selected.kind !== "history" && <div><div className="mb-2 flex items-center gap-2"><Braces className="size-3.5 text-[#75b1ff]" /><h3 className="text-[12px] font-semibold text-white/78">Event data</h3></div><JsonBlock value={selected.data} /></div>}
            {selected.kind === "tool_start" && <p className="flex items-start gap-2 text-[11px] leading-relaxed text-white/38"><TerminalSquare className="mt-0.5 size-3 shrink-0" />The full tool call appears in its model response. The result appears in the next request sent back to the model.</p>}
          </div>}
        </section>
      </div>
    </div>
  );
}

function Metric({ label, value, detail, accent = false }: { label: string; value: string; detail: string; accent?: boolean }) {
  return <div className="min-w-0 rounded-[15px] border border-white/[0.065] bg-white/[0.035] px-3.5 py-3"><p className="text-[10px] font-medium uppercase tracking-[0.1em] text-white/36">{label}</p><p className={cn("mt-1 text-[23px] font-semibold tracking-[-0.05em] text-white/85", accent && "text-[#91c2ff]")}>{value}</p><p className="mt-0.5 truncate text-[10px] text-white/35" title={detail}>{detail}</p></div>;
}

function SmallStat({ label, value }: { label: string; value: string }) {
  return <div className="rounded-xl border border-white/[0.055] bg-black/[0.12] px-3 py-2.5"><p className="text-[10px] text-white/35">{label}</p><p className="mt-0.5 font-mono text-[12px] text-white/75">{value}</p></div>;
}

function MessageList({ rows }: { rows: unknown[] }) {
  return <div className="space-y-2">{rows.map((entry, index) => {
    const row = object(entry);
    const message = object(row?.message);
    return <details key={index} className="group rounded-xl border border-white/[0.06] bg-black/[0.12]"><summary className="cursor-pointer list-none px-3 py-2.5 text-[11px] text-white/68"><span className="mr-2 font-mono text-white/30">{String(index + 1).padStart(2, "0")}</span><span className="capitalize">{string(message?.role) ?? "message"}</span><span className="ml-2 text-white/35">· {string(row?.source) ?? "context"}</span></summary><div className="px-2 pb-2"><JsonBlock value={message} /></div></details>;
  })}</div>;
}
