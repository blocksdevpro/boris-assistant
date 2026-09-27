import { ImageResponse } from "next/og";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

export const alt = "Boris Assistant: open-source voice assistant for Windows";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

const screenshot = await readFile(join(process.cwd(), "public/boris-screenshot.png"), "base64");
const icon = await readFile(join(process.cwd(), "public/boris-icon.png"), "base64");

export default function OpenGraphImage() {
  return new ImageResponse(
    <div style={{ display: "flex", flexDirection: "column", width: "100%", height: "100%", padding: "44px 52px", background: "#090a0c", color: "#f4f4f5", fontFamily: "sans-serif" }}>
      <div style={{ display: "flex", alignItems: "center", gap: 16, fontSize: 30 }}>
        {/* ImageResponse embeds local image bytes directly. */}
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={`data:image/png;base64,${icon}`} width={48} height={48} alt="" />
        <span>Boris Assistant</span>
        <span style={{ marginLeft: "auto", color: "#d9ff75", fontSize: 20 }}>Free and open source</span>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 36, flex: 1 }}>
        <div style={{ display: "flex", flexDirection: "column", width: 430 }}>
          <div style={{ display: "flex", fontSize: 52, lineHeight: 1.1 }}>A voice assistant for Windows.</div>
          <div style={{ display: "flex", marginTop: 24, fontSize: 24, lineHeight: 1.45, color: "#b5b8be" }}>Local speech. File and web tools. Your choice of AI model.</div>
        </div>
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={`data:image/png;base64,${screenshot}`} width={600} height={450} alt="Boris desktop app" style={{ borderRadius: 8 }} />
      </div>
      <div style={{ display: "flex", fontSize: 20, color: "#b5b8be" }}>Windows 10 and 11 · boris.blocksdev.pro</div>
    </div>,
    size,
  );
}
