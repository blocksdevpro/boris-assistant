"use client";

import { useRef, useState, type KeyboardEvent } from "react";
import { AudioLines, Brain, Check, MemoryStick, Mic, ShieldCheck, Sparkles, Volume2, WandSparkles } from "lucide-react";

const steps = [
  {
    label: "Wake",
    short: "Always ready",
    title: "Ready when called. Quiet the rest of the time.",
    copy: "A lightweight wake model runs on your computer. Boris waits for the phrase you choose before the voice pipeline starts.",
    location: "On your PC",
    note: "No continuous cloud audio",
    icon: Sparkles,
  },
  {
    label: "Hear",
    short: "Local speech",
    title: "Your voice becomes text without leaving the room.",
    copy: "Voice activity detection finds the end of your request, then Parakeet transcribes it locally into text Boris can understand.",
    location: "On your PC",
    note: "Audio stays local",
    icon: Mic,
  },
  {
    label: "Think",
    short: "Your model",
    title: "Only the request goes to the model you chose.",
    copy: "Boris routes the transcribed request and useful context through your configured model provider—never raw microphone audio.",
    location: "Your provider",
    note: "Bring your own model",
    icon: Brain,
  },
  {
    label: "Act",
    short: "Safe tools",
    title: "Plans are checked before tools are allowed to run.",
    copy: "Capability presets decide what Boris can access. File, web, and system actions pass through policy and risky work pauses for approval.",
    location: "Policy checked",
    note: "You approve risky actions",
    icon: WandSparkles,
  },
  {
    label: "Remember",
    short: "Local memory",
    title: "Useful context survives the conversation.",
    copy: "Notes, sessions, preferences, and long-term memory live in your Boris home so future requests can pick up where you left off.",
    location: "On your PC",
    note: "Memory is optional",
    icon: MemoryStick,
  },
  {
    label: "Speak",
    short: "Natural voice",
    title: "The answer comes back through a local voice.",
    copy: "Supertone turns Boris’s response into speech on your machine while the voice island shows exactly what is being said.",
    location: "On your PC",
    note: "Local speech output",
    icon: Volume2,
  },
] as const;

export function HowItWorks() {
  const [active, setActive] = useState(0);
  const [trackMoved, setTrackMoved] = useState(false);
  const tabs = useRef<Array<HTMLButtonElement | null>>([]);
  const step = steps[active];
  const Icon = step.icon;

  const moveTo = (index: number) => {
    const next = (index + steps.length) % steps.length;
    setActive(next);
    tabs.current[next]?.focus();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      event.preventDefault();
      moveTo(index + 1);
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      event.preventDefault();
      moveTo(index - 1);
    } else if (event.key === "Home") {
      event.preventDefault();
      moveTo(0);
    } else if (event.key === "End") {
      event.preventDefault();
      moveTo(steps.length - 1);
    }
  };

  return (
    <div className="process-panel">
      <div className={`process-track-wrap${trackMoved ? " process-track-moved" : ""}`}>
        <div
          className="process-track"
          role="tablist"
          aria-label="How Boris handles a voice request"
          onScroll={(event) => {
            if (event.currentTarget.scrollLeft > 4) setTrackMoved(true);
          }}
        >
          {steps.map((item, index) => {
            const ItemIcon = item.icon;
            return (
              <button
                key={item.label}
                ref={(node) => { tabs.current[index] = node; }}
                id={`process-tab-${index}`}
                type="button"
                role="tab"
                tabIndex={index === active ? 0 : -1}
                aria-selected={index === active}
                aria-controls="process-detail"
                className={`process-step${index === active ? " process-active" : ""}${index < active ? " process-complete" : ""}`}
                onClick={() => setActive(index)}
                onKeyDown={(event) => handleKeyDown(event, index)}
              >
                <span className="process-node"><ItemIcon /></span>
                <strong>{item.label}</strong>
                <small>{item.short}</small>
              </button>
            );
          })}
        </div>
      </div>

      <div id="process-detail" className="process-detail" role="tabpanel" aria-labelledby={`process-tab-${active}`} key={step.label}>
        <div className="process-detail-icon"><Icon /></div>
        <div className="process-detail-copy">
          <span>{step.label}</span>
          <h3>{step.title}</h3>
          <p>{step.copy}</p>
        </div>
        <div className="process-context">
          <span><AudioLines /> Signal path</span>
          <strong>{step.location}</strong>
          <small><Check /> {step.note}</small>
          {active === 3 && <small><ShieldCheck /> Human approval available</small>}
        </div>
      </div>
      <p className="process-hint">Follow a request from wake word to response.</p>
    </div>
  );
}
