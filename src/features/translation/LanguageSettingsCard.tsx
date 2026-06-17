import { Languages } from "lucide-react";

import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";

import { LanguageCombobox } from "@/features/languages/LanguageCombobox";
import { displayLanguage } from "@/features/languages/languageOptions";

interface LanguageSettingsCardProps {
  sourceLanguage: string;
  detectedSourceLanguage: string | null;
  targetLanguage: string;
  onSourceLanguageChange: (value: string) => void;
  onTargetLanguageChange: (value: string) => void;
}

function autoLabel(detectedSourceLanguage: string | null): string {
  return detectedSourceLanguage
    ? `自动检测 (${displayLanguage(detectedSourceLanguage)})`
    : "自动检测";
}

export function LanguageSettingsCard({
  sourceLanguage,
  detectedSourceLanguage,
  targetLanguage,
  onSourceLanguageChange,
  onTargetLanguageChange,
}: LanguageSettingsCardProps) {
  return (
    <Card size="sm" className="flex h-full flex-col rounded-[6px] py-3">
      <CardHeader className="px-3">
        <div className="flex items-center gap-2">
          <Languages className="size-4 text-primary" />
          <CardTitle>语言设置</CardTitle>
        </div>
      </CardHeader>
      <CardContent className="grid flex-1 content-start gap-3 px-3">
        <div className="grid gap-2">
          <Label>原始语言</Label>
          <LanguageCombobox
            value={sourceLanguage}
            includeAuto
            autoLabel={autoLabel(detectedSourceLanguage)}
            onValueChange={onSourceLanguageChange}
            placeholder="选择原始语言"
            searchPlaceholder="搜索原始语言"
          />
        </div>
        <div className="grid gap-2">
          <Label>目标语言</Label>
          <LanguageCombobox
            value={targetLanguage}
            onValueChange={onTargetLanguageChange}
            placeholder="选择目标语言"
            searchPlaceholder="搜索目标语言"
          />
        </div>
      </CardContent>
    </Card>
  );
}
