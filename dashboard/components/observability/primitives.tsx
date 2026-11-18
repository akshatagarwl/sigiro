"use client";

import { Button as BaseButton } from "@ui-sdk/react/button";
import { Input as BaseInput } from "@ui-sdk/react/input";
import { Menu } from "@/components/ui/parts";
import type * as React from "react";

import styles from "./observability.module.css";

export function Button({
  className,
  ...props
}: React.ComponentProps<typeof BaseButton>) {
  return (
    <BaseButton
      className={
        typeof className === "function"
          ? (state) => `${styles.button} ${className(state) ?? ""}`
          : `${styles.button} ${className ?? ""}`
      }
      {...props}
    />
  );
}

export function Input({
  className,
  ...props
}: React.ComponentProps<typeof BaseInput>) {
  return (
    <BaseInput
      className={
        typeof className === "function"
          ? (state) => `${styles.input} ${className(state) ?? ""}`
          : `${styles.input} ${className ?? ""}`
      }
      {...props}
    />
  );
}

export function ActionMenu({
  label,
  children,
  actions,
}: {
  label: string;
  children: React.ReactNode;
  actions: { label: string; onClick: () => void; disabled?: boolean }[];
}) {
  return (
    <Menu.Root>
      <Menu.Trigger render={<Button aria-label={label} />}>
        {children}
      </Menu.Trigger>
          <Menu.Popup sideOffset={6} className={styles.menu}>
            {actions.map((action) => (
              <Menu.Item
                key={action.label}
                value={action.label}
                className={styles.menuItem}
                disabled={action.disabled}
                onClick={action.onClick}
              >
                {action.label}
              </Menu.Item>
            ))}
          </Menu.Popup>
    </Menu.Root>
  );
}
