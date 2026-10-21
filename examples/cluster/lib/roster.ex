defmodule Roster do
  @moduledoc """
  Every node runs one `Roster.Member`, which joins the `:roster` process group (`:pg`). Once the
  nodes are connected, each one sees every member, on its own node and on the others.
  """

  @doc "The nodes with a member in the group, as this node sees them."
  def members do
    :pg.get_members(:roster) |> Enum.map(&node/1) |> Enum.uniq() |> Enum.sort()
  end

  @doc "Asks every member, on every node, who it is."
  def roll_call do
    :pg.get_members(:roster) |> Enum.map(&GenServer.call(&1, :hello)) |> Enum.sort()
  end
end

defmodule Roster.Member do
  @moduledoc false
  use GenServer

  def start_link(_), do: GenServer.start_link(__MODULE__, nil)

  @impl true
  def init(nil) do
    :ok = :pg.join(:roster, self())
    {:ok, nil}
  end

  @impl true
  def handle_call(:hello, _from, state), do: {:reply, {:here, node()}, state}
end

defmodule Roster.Peers do
  @moduledoc """
  Connects to the nodes named in `ROSTER_PEERS` (comma-separated), and again whenever a
  connection drops. A node that is not up yet is tried again a second later.
  """
  use GenServer
  require Logger

  def start_link(_), do: GenServer.start_link(__MODULE__, nil, name: __MODULE__)

  @impl true
  def init(nil) do
    peers =
      System.get_env("ROSTER_PEERS", "")
      |> String.split(",", trim: true)
      |> Enum.map(&String.to_atom(String.trim(&1)))

    send(self(), :connect)
    {:ok, peers}
  end

  @impl true
  def handle_info(:connect, peers) do
    for peer <- peers, peer not in Node.list() do
      if Node.connect(peer) == true, do: Logger.info("connected to #{peer}")
    end

    Process.send_after(self(), :connect, 1000)
    {:noreply, peers}
  end
end

defmodule Roster.Application do
  @moduledoc false
  use Application

  @impl true
  def start(_type, _args) do
    children = [%{id: :pg, start: {:pg, :start_link, []}}, Roster.Member, Roster.Peers]
    Supervisor.start_link(children, strategy: :one_for_one, name: Roster.Supervisor)
  end
end
