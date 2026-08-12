import type { Metadata } from "next";
import { Instrument_Sans } from "next/font/google";
import "./globals.css";

const instrument = Instrument_Sans({
  variable: "--font-instrument",
  subsets: ["latin"],
  display: "swap",
});

export const metadata: Metadata = {
  metadataBase: new URL(process.env.NEXT_PUBLIC_SITE_URL ?? "http://localhost:3000"),
  title: "Boris — A voice assistant that can actually help",
  description: "A Windows-first, open-source voice assistant with local speech, useful tools, memory, and approvals.",
  applicationName: "Boris Assistant",
  icons: {
    icon: [
      { url: "/boris-logo.svg", type: "image/svg+xml" },
      { url: "/boris-icon.png", type: "image/png", sizes: "256x256" },
    ],
    shortcut: "/boris-icon.png",
    apple: "/boris-icon.png",
  },
  openGraph: {
    title: "Boris — A voice assistant that can actually help",
    description: "Local voice, your choice of model, and tools that ask before they act.",
    type: "website",
    images: [{ url: "/og.png", width: 1536, height: 1024, alt: "Boris voice assistant" }],
  },
  twitter: {
    card: "summary_large_image",
    title: "Boris — A voice assistant that can actually help",
    description: "Local voice, your choice of model, and tools that ask before they act.",
    images: ["/og.png"],
  },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return <html lang="en" className={instrument.variable}><body>{children}</body></html>;
}
