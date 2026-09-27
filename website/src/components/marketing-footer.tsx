import Image from "next/image";
import Link from "next/link";
import { repo } from "@/lib/site";

export function MarketingFooter() {
  return (
    <footer>
      <div className="mx-auto flex max-w-[1160px] flex-col items-center justify-between gap-5 px-5 py-7 sm:flex-row">
        <Link href="/" className="footer-brand">
          <Image src="/boris-mark.svg" alt="" width={20} height={16} />
          <span>Boris Assistant</span><small>© 2026 BlocksDevPro</small>
        </Link>
        <div className="footer-links">
          <Link href="/download">Download</Link>
          <Link href="/releases">Release notes</Link>
          <Link href="/#faq">FAQ</Link>
          <a href={repo}>GitHub</a>
          <a href={`${repo}/blob/main/SECURITY.md`}>Security</a>
          <a href={`${repo}/blob/main/LICENSE`}>License</a>
        </div>
      </div>
    </footer>
  );
}
