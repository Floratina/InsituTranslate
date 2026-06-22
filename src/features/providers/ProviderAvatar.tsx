import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { cn } from "@/lib/utils";

import { BUILTIN_AVATARS } from "./constants";

type ProviderAvatarSize = "2xs" | "sm" | "md" | "lg" | "xl";

interface ProviderAvatarProps {
  name: string;
  avatar: string | null;
  size?: ProviderAvatarSize;
  className?: string;
}

const avatarSizeClassNames: Record<ProviderAvatarSize, string> = {
  "2xs": "size-3.5",
  sm: "size-7",
  md: "size-8",
  lg: "size-9",
  xl: "size-16",
};

const fallbackSizeClassNames: Record<ProviderAvatarSize, string> = {
  "2xs": "text-3xs",
  sm: "text-xs",
  md: "text-sm",
  lg: "text-base",
  xl: "text-xl",
};

function providerInitial(name: string): string {
  return Array.from(name.trim())[0]?.toUpperCase() ?? "A";
}

export function ProviderAvatar({
  name,
  avatar,
  size = "md",
  className,
}: ProviderAvatarProps) {
  const source = avatar?.startsWith("data:image/") || avatar?.startsWith("/")
    ? avatar
    : avatar && BUILTIN_AVATARS.has(avatar)
      ? `/provider/${avatar}.png`
      : null;

  return (
    <Avatar
      key={source ? "image" : "fallback"}
      className={cn(avatarSizeClassNames[size], className)}
    >
      {source && <AvatarImage src={source} alt="" />}
      <AvatarFallback
        className={cn(
          "bg-primary leading-none font-semibold tracking-normal text-primary-foreground [font-family:var(--avatar-letter-font-family)] [&>span]:-translate-y-[0.04em]",
          fallbackSizeClassNames[size],
        )}
      >
        <span>{providerInitial(name)}</span>
      </AvatarFallback>
    </Avatar>
  );
}
