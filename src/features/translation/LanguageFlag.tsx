import { Globe2 } from "lucide-react";

import { cn } from "@/lib/utils";

export type LanguageFlagCode =
  | "cn"
  | "hk"
  | "gb"
  | "jp"
  | "fr"
  | "de"
  | "ru"
  | "es"
  | "kr"
  | "pt"
  | "br"
  | "sa"
  | "vn"
  | "it"
  | "nl"
  | "pl"
  | "ua"
  | "tr"
  | "ir"
  | "in"
  | "bd"
  | "th"
  | "id"
  | "my"
  | "ph"
  | "se"
  | "no"
  | "dk"
  | "fi"
  | "cz"
  | "ro"
  | "hu"
  | "gr"
  | "il"
  | "va"
  | "other";

interface LanguageFlagProps {
  code: LanguageFlagCode;
  className?: string;
}

export function LanguageFlag({ code, className }: LanguageFlagProps) {
  if (code === "other") {
    return <Globe2 className={cn("size-4 text-primary", className)} />;
  }

  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 16"
      className={cn("h-3.5 w-[21px] shrink-0 overflow-hidden rounded-[2px] shadow-[0_0_0_1px_rgba(127,127,127,0.3)]", className)}
    >
      {code === "cn" && (
        <>
          <rect width="24" height="16" fill="#de2910" />
          <text x="3.1" y="7.2" fill="#ffde00" fontSize="6.5">★</text>
        </>
      )}
      {code === "hk" && (
        <>
          <rect width="24" height="16" fill="#de2910" />
          <text x="12" y="11.2" fill="#fff" fontSize="9" textAnchor="middle">✿</text>
        </>
      )}
      {code === "gb" && (
        <>
          <rect width="24" height="16" fill="#012169" />
          <path d="M0 0 24 16M24 0 0 16" stroke="#fff" strokeWidth="3.6" />
          <path d="M0 0 24 16M24 0 0 16" stroke="#c8102e" strokeWidth="1.5" />
          <path d="M12 0v16M0 8h24" stroke="#fff" strokeWidth="5" />
          <path d="M12 0v16M0 8h24" stroke="#c8102e" strokeWidth="2.6" />
        </>
      )}
      {code === "jp" && (
        <>
          <rect width="24" height="16" fill="#fff" />
          <circle cx="12" cy="8" r="4.2" fill="#bc002d" />
        </>
      )}
      {code === "fr" && (
        <>
          <rect width="8" height="16" fill="#002395" />
          <rect x="8" width="8" height="16" fill="#fff" />
          <rect x="16" width="8" height="16" fill="#ed2939" />
        </>
      )}
      {code === "de" && (
        <>
          <rect width="24" height="5.34" fill="#000" />
          <rect y="5.33" width="24" height="5.34" fill="#dd0000" />
          <rect y="10.66" width="24" height="5.34" fill="#ffce00" />
        </>
      )}
      {code === "ru" && (
        <>
          <rect width="24" height="5.34" fill="#fff" />
          <rect y="5.33" width="24" height="5.34" fill="#0039a6" />
          <rect y="10.66" width="24" height="5.34" fill="#d52b1e" />
        </>
      )}
      {code === "es" && (
        <>
          <rect width="24" height="16" fill="#aa151b" />
          <rect y="4" width="24" height="8" fill="#f1bf00" />
          <rect x="6" y="6" width="2" height="4" fill="#aa151b" />
        </>
      )}
      {code === "kr" && (
        <>
          <rect width="24" height="16" fill="#fff" />
          <path d="M8 8a4 4 0 0 1 8 0 2 2 0 0 1-4 0 2 2 0 0 0-4 0Z" fill="#cd2e3a" />
          <path d="M16 8a4 4 0 0 1-8 0 2 2 0 0 1 4 0 2 2 0 0 0 4 0Z" fill="#0047a0" />
          <path d="m4 4 3 2m10 4 3 2M4 12l3-2m10-4 3-2" stroke="#111" strokeWidth="1" />
        </>
      )}
      {code === "pt" && (
        <>
          <rect x="0" width="9.5" height="16" fill="#006600" />
          <rect x="9.5" width="14.5" height="16" fill="#ff0000" />
          <circle cx="9.5" cy="8" r="2.4" fill="#ffcc00" />
        </>
      )}
      {code === "br" && (
        <>
          <rect width="24" height="16" fill="#009b3a" />
          <path d="M12 2.6 21 8l-9 5.4L3 8Z" fill="#ffdf00" />
          <circle cx="12" cy="8" r="3" fill="#002776" />
        </>
      )}
      {code === "sa" && (
        <>
          <rect width="24" height="16" fill="#006c35" />
          <path d="M6 11h12" stroke="#fff" strokeWidth="1" />
          <text x="12" y="8.3" fill="#fff" fontSize="4" textAnchor="middle">الله</text>
        </>
      )}
      {code === "vn" && (
        <>
          <rect width="24" height="16" fill="#da251d" />
          <text x="12" y="11.2" fill="#ff0" fontSize="9" textAnchor="middle">★</text>
        </>
      )}
      {code === "it" && (
        <>
          <rect width="8" height="16" fill="#009246" />
          <rect x="8" width="8" height="16" fill="#fff" />
          <rect x="16" width="8" height="16" fill="#ce2b37" />
        </>
      )}
      {code === "nl" && (
        <>
          <rect width="24" height="5.34" fill="#ae1c28" />
          <rect y="5.33" width="24" height="5.34" fill="#fff" />
          <rect y="10.66" width="24" height="5.34" fill="#21468b" />
        </>
      )}
      {code === "pl" && (
        <>
          <rect width="24" height="8" fill="#fff" />
          <rect y="8" width="24" height="8" fill="#dc143c" />
        </>
      )}
      {code === "ua" && (
        <>
          <rect width="24" height="8" fill="#0057b7" />
          <rect y="8" width="24" height="8" fill="#ffd700" />
        </>
      )}
      {code === "tr" && (
        <>
          <rect width="24" height="16" fill="#e30a17" />
          <circle cx="10" cy="8" r="4" fill="#fff" />
          <circle cx="11.2" cy="8" r="3.2" fill="#e30a17" />
          <text x="15.2" y="10.5" fill="#fff" fontSize="5" textAnchor="middle">★</text>
        </>
      )}
      {code === "ir" && (
        <>
          <rect width="24" height="5.34" fill="#239f40" />
          <rect y="5.33" width="24" height="5.34" fill="#fff" />
          <rect y="10.66" width="24" height="5.34" fill="#da0000" />
        </>
      )}
      {code === "in" && (
        <>
          <rect width="24" height="5.34" fill="#ff9933" />
          <rect y="5.33" width="24" height="5.34" fill="#fff" />
          <rect y="10.66" width="24" height="5.34" fill="#138808" />
          <circle cx="12" cy="8" r="1.6" fill="none" stroke="#000080" strokeWidth="0.7" />
        </>
      )}
      {code === "bd" && (
        <>
          <rect width="24" height="16" fill="#006a4e" />
          <circle cx="10.6" cy="8" r="4" fill="#f42a41" />
        </>
      )}
      {code === "th" && (
        <>
          <rect width="24" height="16" fill="#a51931" />
          <rect y="2.7" width="24" height="10.6" fill="#f4f5f8" />
          <rect y="5.3" width="24" height="5.4" fill="#2d2a4a" />
        </>
      )}
      {code === "id" && (
        <>
          <rect width="24" height="8" fill="#ce1126" />
          <rect y="8" width="24" height="8" fill="#fff" />
        </>
      )}
      {code === "my" && (
        <>
          <rect width="24" height="16" fill="#fff" />
          {Array.from({ length: 4 }).map((_, index) => (
            <rect key={index} y={index * 4} width="24" height="2" fill="#cc0001" />
          ))}
          <rect width="10" height="8" fill="#010066" />
          <circle cx="4.4" cy="4" r="2.3" fill="#ffcc00" />
          <circle cx="5.1" cy="4" r="1.9" fill="#010066" />
        </>
      )}
      {code === "ph" && (
        <>
          <rect width="24" height="8" fill="#0038a8" />
          <rect y="8" width="24" height="8" fill="#ce1126" />
          <path d="M0 0v16l9-8Z" fill="#fff" />
          <circle cx="3.5" cy="8" r="1" fill="#fcd116" />
        </>
      )}
      {code === "se" && (
        <>
          <rect width="24" height="16" fill="#006aa7" />
          <path d="M0 7h24M8 0v16" stroke="#fecc00" strokeWidth="3" />
        </>
      )}
      {code === "no" && (
        <>
          <rect width="24" height="16" fill="#ba0c2f" />
          <path d="M0 7h24M8 0v16" stroke="#fff" strokeWidth="4" />
          <path d="M0 7h24M8 0v16" stroke="#00205b" strokeWidth="2" />
        </>
      )}
      {code === "dk" && (
        <>
          <rect width="24" height="16" fill="#c60c30" />
          <path d="M0 7h24M8 0v16" stroke="#fff" strokeWidth="2.4" />
        </>
      )}
      {code === "fi" && (
        <>
          <rect width="24" height="16" fill="#fff" />
          <path d="M0 7h24M8 0v16" stroke="#002f6c" strokeWidth="3" />
        </>
      )}
      {code === "cz" && (
        <>
          <rect width="24" height="8" fill="#fff" />
          <rect y="8" width="24" height="8" fill="#d7141a" />
          <path d="M0 0v16l11-8Z" fill="#11457e" />
        </>
      )}
      {code === "ro" && (
        <>
          <rect width="8" height="16" fill="#002b7f" />
          <rect x="8" width="8" height="16" fill="#fcd116" />
          <rect x="16" width="8" height="16" fill="#ce1126" />
        </>
      )}
      {code === "hu" && (
        <>
          <rect width="24" height="5.34" fill="#ce2939" />
          <rect y="5.33" width="24" height="5.34" fill="#fff" />
          <rect y="10.66" width="24" height="5.34" fill="#477050" />
        </>
      )}
      {code === "gr" && (
        <>
          <rect width="24" height="16" fill="#0d5eaf" />
          {Array.from({ length: 4 }).map((_, index) => (
            <rect key={index} y={index * 4 + 2} width="24" height="2" fill="#fff" />
          ))}
          <rect width="8" height="8" fill="#0d5eaf" />
          <path d="M0 4h8M4 0v8" stroke="#fff" strokeWidth="1.6" />
        </>
      )}
      {code === "il" && (
        <>
          <rect width="24" height="16" fill="#fff" />
          <rect y="2" width="24" height="2" fill="#0038b8" />
          <rect y="12" width="24" height="2" fill="#0038b8" />
          <path d="M12 5 9.4 10h5.2L12 5Zm0 6 2.6-5H9.4L12 11Z" fill="none" stroke="#0038b8" strokeWidth="0.8" />
        </>
      )}
      {code === "va" && (
        <>
          <rect width="12" height="16" fill="#ffe000" />
          <rect x="12" width="12" height="16" fill="#fff" />
          <circle cx="16.8" cy="8" r="2" fill="none" stroke="#b7a369" strokeWidth="0.8" />
        </>
      )}
    </svg>
  );
}
