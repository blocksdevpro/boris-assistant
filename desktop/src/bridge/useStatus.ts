/**
 * React hook: live engine status for main window + overlay.
 *
 * Subscribes to host `status` events; initial snapshot via `get_status`.
 * The host mirrors pipeline `StatusPicture` — this hook is pure UI glue.
 */

import { useEffect, useState } from "react";
import { getStatus, onStatus } from "./status";
import { OFF_STATUS, type StatusPicture } from "./types";

/** Live engine status for main window + overlay. */
export function useStatus(): StatusPicture {
  const [status, setStatus] = useState<StatusPicture>(OFF_STATUS);

  useEffect(() => {
    let active = true;
    let unsub = () => {};

    void getStatus().then((s) => {
      if (active) setStatus(s);
    });

    void onStatus((s) => {
      if (active) setStatus(s);
    }).then((u) => {
      unsub = u;
    });

    return () => {
      active = false;
      unsub();
    };
  }, []);

  return status;
}
