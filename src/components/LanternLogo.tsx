interface LanternLogoProps {
  size?: number;
  className?: string;
}

export default function LanternLogo({ size = 32, className = "" }: LanternLogoProps) {
  return (
    <img
      src="/app-icon.png"
      alt=""
      width={size}
      height={size}
      className={className}
      aria-hidden="true"
    />
  );
}
