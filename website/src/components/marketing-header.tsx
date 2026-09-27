"use client";

import { useEffect, useState } from "react";
import { ArrowUpRight, Star } from "lucide-react";
import Image from "next/image";
import Link from "next/link";
import { repo } from "@/lib/site";

function Mark() {
  return <Image src="/boris-mark.svg" alt="" width={24} height={24} className="header-mark" aria-hidden="true" />;
}

export function MarketingHeader() {
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const update = () => setScrolled(window.scrollY > 18);
    update();
    window.addEventListener("scroll", update, { passive: true });
    return () => window.removeEventListener("scroll", update);
  }, []);

  return (
    <header className={`marketing-header ${scrolled ? "header-scrolled" : ""}`}>
      <nav className="marketing-nav" aria-label="Primary navigation">
        <Link href="/" className="header-brand" aria-label="Boris Assistant home"><Mark /><span>Boris</span></Link>
        <div className="header-links">
          <Link href="/#features">Features</Link>
          <Link href="/download">Download</Link>
          <Link href="/releases">Releases</Link>
        </div>
        <a href={repo} target="_blank" rel="noreferrer" className="github-pill">
          <Star className="github-star" fill="currentColor" />
          <span><strong>Star on GitHub</strong><small>Open source</small></span>
          <ArrowUpRight className="github-arrow" />
        </a>
      </nav>
    </header>
  );
}
