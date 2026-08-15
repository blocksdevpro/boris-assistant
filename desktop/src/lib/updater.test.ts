import { describe, expect, it } from "vitest";
import { normalizeSettings, normalizeUpdateChannel } from "@/bridge/types";
import {
  BETA_UPDATE_ENDPOINT,
  endpointForChannel,
  STABLE_UPDATE_ENDPOINT,
} from "./updater";

describe("update channel", () => {
  it("defaults missing or unknown values to stable", () => {
    expect(normalizeUpdateChannel(undefined)).toBe("stable");
    expect(normalizeUpdateChannel("")).toBe("stable");
    expect(normalizeUpdateChannel("nightly")).toBe("stable");
    expect(normalizeUpdateChannel("BETA")).toBe("beta");
  });

  it("maps the channel to the matching GitHub feed", () => {
    expect(endpointForChannel("stable")).toBe(STABLE_UPDATE_ENDPOINT);
    expect(endpointForChannel("beta")).toBe(BETA_UPDATE_ENDPOINT);
    expect(endpointForChannel(undefined)).toBe(STABLE_UPDATE_ENDPOINT);
    expect(STABLE_UPDATE_ENDPOINT).toContain("/releases/latest/download/latest.json");
    expect(BETA_UPDATE_ENDPOINT).toContain("/releases/download/beta/latest.json");
  });

  it("normalizes settings onto the stable channel by default", () => {
    expect(normalizeSettings({}).update_channel).toBe("stable");
    expect(normalizeSettings({ update_channel: "beta" }).update_channel).toBe(
      "beta",
    );
    expect(normalizeSettings({}).start_with_windows).toBe(false);
    expect(normalizeSettings({ start_with_windows: true }).start_with_windows).toBe(
      true,
    );
  });
});
