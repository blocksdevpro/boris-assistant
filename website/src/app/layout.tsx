import type { Metadata } from "next";
import { Instrument_Sans } from "next/font/google";
import { MarketingHeader } from "@/components/marketing-header";
import { MarketingFooter } from "@/components/marketing-footer";
import { pageMetadata, siteDescription, siteName, siteTitle, siteUrl } from "@/lib/site";
import "./globals.css";

const instrument = Instrument_Sans({
  variable: "--font-instrument",
  subsets: ["latin"],
  display: "swap",
});

export const metadata: Metadata = {
  ...pageMetadata({ title: siteTitle, description: siteDescription, path: "/" }),
  metadataBase: new URL(siteUrl),
  applicationName: siteName,
  icons: {
    icon: [
      { url: "/boris-logo.svg", type: "image/svg+xml" },
      { url: "/boris-icon.png", type: "image/png", sizes: "256x256" },
    ],
    shortcut: "/boris-icon.png",
    apple: "/boris-icon.png",
  },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" className={instrument.variable}>
      <body>
        <a href="#main-content" className="skip-link">Skip to content</a>
        <MarketingHeader />
        {children}
        <MarketingFooter />
      </body>
    </html>
  );
}
