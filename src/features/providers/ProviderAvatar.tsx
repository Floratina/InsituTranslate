import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";

import { BUILTIN_AVATARS } from "./constants";

interface ProviderAvatarProps {
  name: string;
  avatar: string | null;
  className?: string;
}

function providerInitial(name: string): string {
  return Array.from(name.trim())[0]?.toUpperCase() ?? "A";
}

export function ProviderAvatar({ name, avatar, className }: ProviderAvatarProps) {
  const source = avatar?.startsWith("data:image/") || avatar?.startsWith("/")
    ? avatar
    : avatar && BUILTIN_AVATARS.has(avatar)
      ? `/provider/${avatar}.png`
      : null;

  return (
    <Avatar key={source ? "image" : "fallback"} className={className}>
      {source && <AvatarImage src={source} alt="" />}
      <AvatarFallback className="bg-primary font-bold text-primary-foreground">
        {providerInitial(name)}
      </AvatarFallback>
    </Avatar>
  );
}
