import { useMemo, useRef, useState } from "react";
import { ImageUp, Search, Trash2 } from "lucide-react";
import { motion } from "motion/react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

import { AVATAR_LIBRARY } from "./constants";
import { ProviderAvatar } from "./ProviderAvatar";

interface AvatarPickerPopoverProps {
  name: string;
  avatar: string | null;
  onAvatarChange: (avatar: string | null) => void;
  onError: (message: string) => void;
}

const contentTransition = { duration: 0.22, ease: [0.03, 0.59, 0.19, 1] as const };

export function AvatarPickerPopover({
  name,
  avatar,
  onAvatarChange,
  onError,
}: AvatarPickerPopoverProps) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const filteredAvatars = useMemo(
    () =>
      AVATAR_LIBRARY.filter((item) =>
        item.name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()),
      ),
    [search],
  );

  function readAvatar(file: File | undefined): void {
    if (!file) return;
    if (!file.type.startsWith("image/")) {
      onError("请选择图片文件");
      return;
    }
    if (file.size > 1024 * 1024) {
      onError("头像图片不能超过 1 MB");
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      if (typeof reader.result === "string") {
        onAvatarChange(reader.result);
        setOpen(false);
      }
    };
    reader.onerror = () => onError("头像图片读取失败");
    reader.readAsDataURL(file);
  }

  return (
    <Popover modal open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <motion.button
          type="button"
          whileHover={{ scale: 1.04 }}
          whileTap={{ scale: 0.97 }}
          transition={{ duration: 0.2, ease: [0.03, 0.59, 0.19, 1] }}
          className="rounded-full outline-none focus-visible:ring-3 focus-visible:ring-ring/35"
          title="设置头像"
        >
          <ProviderAvatar name={name} avatar={avatar} size="xl" />
        </motion.button>
      </PopoverTrigger>
      <PopoverContent className="w-[500px] p-2" align="center">
        <input
          ref={fileInputRef}
          type="file"
          accept="image/*"
          className="hidden"
          onChange={(event) => {
            readAvatar(event.target.files?.[0]);
            event.target.value = "";
          }}
        />
        <Tabs defaultValue="upload" className="gap-2">
          <TabsList className="w-full">
            <TabsTrigger value="upload">图片上传</TabsTrigger>
            <TabsTrigger value="builtin">内置头像</TabsTrigger>
          </TabsList>
          <TabsContent value="upload">
            <motion.button
              type="button"
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              transition={contentTransition}
              className="flex h-28 w-full flex-col items-center justify-center gap-2 rounded-[6px] border border-dashed text-center text-muted-foreground hover:border-ring hover:bg-accent/40 hover:text-primary"
              onClick={() => fileInputRef.current?.click()}
            >
              <span className="inline-flex items-center gap-1.5 text-sm font-medium">
                <ImageUp className="size-4" strokeWidth={1.8} />
                选择图片
              </span>
            </motion.button>
          </TabsContent>
          <TabsContent value="builtin">
            <motion.div
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              transition={contentTransition}
              className="grid gap-2"
            >
              <div className="relative">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={search}
                  className="pl-8"
                  placeholder="搜索内置头像"
                  onChange={(event) => setSearch(event.target.value)}
                />
              </div>
              <div className="scrollbar-subtle h-52 overflow-x-hidden overflow-y-auto overscroll-contain">
                <div className="grid grid-cols-2 gap-1">
                  {filteredAvatars.map((item) => (
                    <button
                      key={item.src}
                      type="button"
                      className={cn(
                        "flex h-11 min-w-0 items-center gap-2 rounded-[6px] border px-2 text-left hover:bg-accent",
                        avatar === item.src && "border-enabled-accent/40 bg-enabled-accent/10",
                      )}
                      onClick={() => {
                        onAvatarChange(item.src);
                        setOpen(false);
                      }}
                    >
                      <ProviderAvatar name={item.name} avatar={item.src} size="sm" />
                      <span className="truncate text-xs font-medium">{item.name}</span>
                    </button>
                  ))}
                  {filteredAvatars.length === 0 && (
                    <div className="col-span-2 p-5 text-center text-xs text-muted-foreground">
                      没有匹配的内置头像
                    </div>
                  )}
                </div>
              </div>
            </motion.div>
          </TabsContent>
        </Tabs>
        <div className="mt-2 flex justify-end border-t pt-2">
          <Button
            variant="destructive"
            onClick={() => {
              onAvatarChange(null);
              setOpen(false);
            }}
          >
            <Trash2 className="size-3.5" />
            清除头像
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
