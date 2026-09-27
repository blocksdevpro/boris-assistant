import type { Metadata } from "next";

export const siteUrl = new URL(
  process.env.NEXT_PUBLIC_SITE_URL || "https://boris.blocksdev.pro",
).origin;
export const repo = "https://github.com/blocksdevpro/boris-assistant";
export const siteName = "Boris Assistant";
export const siteTitle = "Boris Assistant | Open-source voice assistant for Windows";
export const siteDescription =
  "Boris is a free, open-source AI voice assistant for Windows 10 and 11. Local speech, file and web tools, optional memory, and your choice of AI model.";

export function pageMetadata({
  title,
  description,
  path,
}: {
  title: string;
  description: string;
  path: string;
}): Metadata {
  const url = new URL(path, siteUrl).href;
  const image = {
    url: "/opengraph-image",
    width: 1200,
    height: 630,
    alt: "Boris Assistant for Windows with the desktop app and local voice features",
  };

  return {
    title,
    description,
    alternates: { canonical: url },
    openGraph: {
      title,
      description,
      url,
      siteName,
      locale: "en_US",
      type: "website",
      images: [image],
    },
    twitter: {
      card: "summary_large_image",
      title,
      description,
      images: [image],
    },
  };
}
