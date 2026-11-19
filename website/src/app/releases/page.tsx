import { ArrowRight } from "lucide-react";
import Link from "next/link";
import { betaRelease, stableRelease } from "@/lib/releases";
import { pageMetadata, repo } from "@/lib/site";

export const metadata = pageMetadata({
  title: "Boris Assistant release notes | Stable and beta updates",
  description: `See what's new in Boris Assistant stable ${stableRelease.version}, including live transcription, tool progress, local memory, and Windows downloads. Preview beta ${betaRelease.version} adds task-aware tool selection and parallel tool calls.`,
  path: "/releases",
});

export default function ReleasesPage() {
  return (
    <main id="main-content" className="content-page section-shell">
      <p className="story-label"><span /> Published updates</p>
      <h1>Boris Assistant release notes</h1>
      <p className="page-intro">Boris {stableRelease.version} is the latest stable release. Beta {betaRelease.version} previews the next version.</p>

      {[stableRelease, betaRelease].map((release) => (
        <article key={release.version} id={release.channel.toLowerCase()} className="release-entry">
          <div className="release-date">
            <span className={`channel-label ${release.channel === "Beta" ? "beta-label" : ""}`}>{release.channel}</span>
            <time dateTime={release.date}>{release.dateLabel}</time>
          </div>
          <div>
            <h2>Boris {release.version}</h2>
            <p className="release-lead">{release.summary}</p>
            <ul>{release.highlights.map((highlight) => <li key={highlight}>{highlight}</li>)}</ul>
            {release.channel === "Stable" && (
              <aside className="migration-note">
                <h3>Memory migration and privacy</h3>
                <p>When upgrading from beta.1 or an older version, Boris refines historical memory using the LLM provider you configured, then moves it to a local SQLite store. Original files are retired only after refinement and verification succeed. Failed imports leave them intact for a later retry. Upgrades from beta.2 or beta.3 keep the existing local store.</p>
                <p>To disable canonical memory and skip migration, set <code>BORIS_MEMORY=0</code> before your first launch of beta.2 or later.</p>
              </aside>
            )}
            {release.channel === "Beta" && (
              <aside className="migration-note">
                <h3>Upgrading and your data</h3>
                <p>Upgrades from {stableRelease.version} keep the existing local SQLite memory store. Upgrades from 1.2.0-beta.1 or earlier still run the verified one-time migration described in the stable notes, which uses the LLM provider you configured.</p>
                <p>Your <code>%USERPROFILE%\.boris</code> data stays outside the installation directory, so installing the beta over stable keeps sessions, memory, and settings.</p>
              </aside>
            )}
            <div className="release-links">
              <Link href={release.channel === "Beta" ? "/download#beta" : "/download"} className="text-link">Download {release.channel.toLowerCase()} <ArrowRight size={16} /></Link>
              <a href={release.url} className="text-link">Full notes on GitHub <ArrowRight size={16} /></a>
            </div>
          </div>
        </article>
      ))}
      <a href={`${repo}/releases`} className="text-link">All published releases on GitHub <ArrowRight size={16} /></a>
    </main>
  );
}
