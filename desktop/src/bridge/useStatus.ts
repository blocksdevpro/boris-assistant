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
import { getStatus, onStatus } from "./status";
import { OFF_STATUS, type StatusPicture } from "./types";

const StatusPreviewContext = createContext<StatusPicture | null>(null);

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
  const [status, setStatus] = useState<StatusPicture>(
    () => providedPreviewStatus ?? OFF_STATUS,
  );

  useEffect(() => {
    let active = true;
    let unsub = () => {};

    if (providedPreviewStatus) {
      setStatus(providedPreviewStatus);
      return () => {
        active = false;
      };
    }

    void getStatus()
      .then((s) => {
        if (active) setStatus(s);
      })
      .catch(() => {
        // Plain-browser previews do not expose the native command bridge.
      });

    void onStatus((s) => {
      if (active) setStatus(s);
    })
      .then((u) => {
        unsub = u;
      })
      .catch(() => {
        // Plain-browser previews do not expose the native event bridge.
      });

    return () => {
      active = false;
      unsub();
    };
  }, [providedPreviewStatus]);

  return status;
}
