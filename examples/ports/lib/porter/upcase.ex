defmodule Porter.Upcase do
  @moduledoc """
  A long-running host program behind a port: a shell loop that upper-cases each line it reads.

  The GenServer owns the port. Requests go out as lines on the program's stdin and answers come
  back as lines on its stdout, in order. If the program exits, the GenServer stops with it and
  the supervisor starts both again, as it would with any port program on a native release.
  """
  use GenServer

  @script ~S(while IFS= read -r line; do printf '%s\n' "$line" | tr '[:lower:]' '[:upper:]'; done)

  def start_link(_), do: GenServer.start_link(__MODULE__, nil, name: __MODULE__)

  @doc "The text, upper-cased by the host program."
  def upcase(text), do: GenServer.call(__MODULE__, {:upcase, text})

  @doc "The host pid of the program behind the port."
  def os_pid, do: GenServer.call(__MODULE__, :os_pid)

  @impl true
  def init(nil) do
    port = Port.open({:spawn_executable, "/bin/sh"}, [:binary, :exit_status, {:line, 4096}, args: ["-c", @script]])
    {:ok, %{port: port, waiting: :queue.new()}}
  end

  @impl true
  def handle_call({:upcase, text}, from, state) do
    Port.command(state.port, [text, ?\n])
    {:noreply, %{state | waiting: :queue.in(from, state.waiting)}}
  end

  def handle_call(:os_pid, _from, state) do
    {:os_pid, pid} = Port.info(state.port, :os_pid)
    {:reply, pid, state}
  end

  @impl true
  def handle_info({port, {:data, {:eol, line}}}, %{port: port} = state) do
    {{:value, from}, waiting} = :queue.out(state.waiting)
    GenServer.reply(from, line)
    {:noreply, %{state | waiting: waiting}}
  end

  def handle_info({port, {:exit_status, status}}, %{port: port} = state) do
    {:stop, {:program_exited, status}, state}
  end
end
