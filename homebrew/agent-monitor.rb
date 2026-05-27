cask "agent-monitor" do
  version "0.3.0"

  on_intel do
    sha256 :no_check
    url "https://github.com/Hoemr/agent-monitor/releases/download/v#{version}/agent-monitor-x86_64-apple-darwin"
  end
  on_arm do
    sha256 :no_check
    url "https://github.com/Hoemr/agent-monitor/releases/download/v#{version}/agent-monitor-aarch64-apple-darwin"
  end

  name "Agent Monitor"
  desc "A pixel-mechanical floating desktop widget for real-time AI coding agent session monitoring"
  homepage "https://github.com/Hoemr/agent-monitor"

  depends_on macos: ">= :ventura"

  app "agent-monitor"
end
