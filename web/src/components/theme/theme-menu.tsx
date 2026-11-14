"use client";

import { MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { applyTheme, getChoice, THEME_CHOICES, type ThemeChoice } from "@/lib/theme";
import { useThemeChoice } from "@/lib/use-theme";

/** The words, one place: the menus and the tests read them. */
export const THEME_LABEL = "Theme";
const WORDS: Record<ThemeChoice, { label: string; icon: typeof SunIcon }> = {
  system: { label: "System", icon: MonitorIcon },
  light: { label: "Light", icon: SunIcon },
  dark: { label: "Dark", icon: MoonIcon },
};

/**
 * Keeps the page on the chosen theme after it hydrates (the address bar's colour, which the head script cannot reach
 * before the metas exist) and follows the device while the choice is System. Mounted once, in the layout.
 */
export function ThemeSync() {
  // subscribing is what keeps the device (System) and other tabs followed; `lib/theme.ts` applies each change itself
  useThemeChoice();
  useEffect(() => {
    applyTheme(getChoice());
  }, []);
  return null;
}

/**
 * The three themes as a group of radio items, to go inside a menu: the current one is checked, the arrow keys move
 * between them, and choosing one changes the page at once and keeps the menu open (nothing moves: the group is the
 * same size in every state).
 */
export function ThemeRadioGroup() {
  const [choice, setChoice] = useThemeChoice();
  return (
    <>
      <DropdownMenuLabel id="theme-group-label">{THEME_LABEL}</DropdownMenuLabel>
      <DropdownMenuRadioGroup
        aria-labelledby="theme-group-label"
        value={choice}
        onValueChange={(value) => setChoice(value as ThemeChoice)}
      >
        {THEME_CHOICES.map((value) => {
          const { label, icon: Icon } = WORDS[value];
          return (
            <DropdownMenuRadioItem
              key={value}
              value={value}
              // the person sees the page change; the menu stays until they are done
              onSelect={(event) => event.preventDefault()}
              className="items-center py-1.5"
            >
              <Icon aria-hidden="true" className="size-4 text-muted-foreground" />
              {label}
            </DropdownMenuRadioItem>
          );
        })}
      </DropdownMenuRadioGroup>
    </>
  );
}

/**
 * The theme as a button of its own, where there is no account menu to hold it: the sign-in screen, a shared
 * conversation, and a local run with nobody signed in.
 */
export function ThemeMenu({ className }: { className?: string }) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          data-slot="theme-menu"
          aria-label={THEME_LABEL}
          title={THEME_LABEL}
          className={className ?? "size-9 rounded-full text-muted-foreground"}
        >
          {/* the icon follows the page's theme by CSS: the server's render and the first client render agree */}
          <SunIcon aria-hidden="true" className="size-4.5 dark:hidden" />
          <MoonIcon aria-hidden="true" className="hidden size-4.5 dark:block" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-44">
        <ThemeRadioGroup />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
