/**
 * React hook: live engine status for main window + overlay.
 *
 * Subscribes to host `status` events; initial snapshot via `get_status`.
 * The host mirrors pipeline `StatusPicture` — this hook is pure UI glue.
 */

import {
  createContext,
  createElement,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { getStatus, onStatus } from "./commands/engine";
import { OFF_STATUS, type StatusPicture } from "./types";

const StatusPreviewContext = createContext<StatusPicture | null>(null);

/** Tauri assigns a process-wide sequence after ordering pipeline snapshots. */
export function latestStatus(current: StatusPicture, incoming: StatusPicture): StatusPicture {
  return (incoming.seq ?? 0) > (current.seq ?? 0) ? incoming : current;
}

/**
 * Injects a deterministic status into a browser-only visual fixture.
 * Product windows never mount this provider.
 */
export function StatusPreviewProvider({
  status,
  children,
}: {
  status: StatusPicture;
  children: ReactNode;
}) {
  return createElement(
    StatusPreviewContext.Provider,
    { value: status },
    children,
  );
}

/** Live engine status for main window + overlay. */
export function useStatus(): StatusPicture {
  const providedPreviewStatus = useContext(StatusPreviewContext);
  const [status, setStatus] = useState<StatusPicture>(OFF_STATUS);

  useEffect(() => {
    let active = true;
    let unsub = () => {};

    if (providedPreviewStatus) {
      return () => {
        active = false;
      };
    }

    void onStatus((s) => {
      if (active) setStatus((current) => latestStatus(current, s));
    })
      .then((u) => {
        if (!active) {
          u();
          return;
        }
        unsub = u;
        // Subscribe first, then pull. The sequence handles either arrival order.
        void getStatus().then((s) => {
          if (active) setStatus((current) => latestStatus(current, s));
        });
      })
      .catch(() => {
        // Plain-browser previews do not expose the native event bridge.
      });

    return () => {
      active = false;
      unsub();
    };
  }, [providedPreviewStatus]);

  // Preview state comes straight from its provider so interactive state changes
  // do not render one frame of the previous snapshot before the effect runs.
  return providedPreviewStatus ?? status;
}
