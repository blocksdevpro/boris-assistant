import type { MetadataRoute } from "next";
import { betaRelease, stableRelease } from "@/lib/releases";
import { siteUrl } from "@/lib/site";

// Dates change only when page content does, so crawlers can trust them.
const latestRelease = [stableRelease.date, betaRelease.date].sort().at(-1)!;
const contentUpdated = "2026-11-17";

export default function sitemap(): MetadataRoute.Sitemap {
  const pages: [string, string][] = [
    ["/", latestRelease],
    ["/download", latestRelease],
    ["/releases", latestRelease],
    ["/privacy", contentUpdated],
    ["/cortana-alternative", contentUpdated],
  ];
  return pages.map(([path, lastModified]) => ({
    url: new URL(path, siteUrl).href,
    lastModified,
  }));
}
