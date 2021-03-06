-module(distributed_server).

%% this file was generated by grpc

-export([decoder/0,
         'CheckIn'/3,
         'Setup'/3,
         'Cleanup'/3,
         'Shutdown'/3]).

-type 'ClientInfo'() ::
    #{address => string(),
      port => integer()}.

-type 'CheckinResponse'() ::
    #{}.

-type 'SetupConfig'() ::
    #{label => string(),
      data => string()}.

-type 'SetupResponse'() ::
    #{success => boolean(),
      data => string()}.

-type 'CleanupInfo'() ::
    #{final => boolean()}.

-type 'CleanupResponse'() ::
    #{}.

-type 'TestResult'() ::
    #{sealed_value =>
          {success, 'TestSuccess'()} |
          {failure, 'TestFailure'()} |
          {not_implemented, 'NotImplemented'()}}.

-type 'TestSuccess'() ::
    #{number_of_runs => integer(),
      run_results => [float() | infinity | '-infinity' | nan]}.

-type 'TestFailure'() ::
    #{reason => string()}.

-type 'NotImplemented'() ::
    #{}.

-type 'ReadyRequest'() ::
    #{}.

-type 'ReadyResponse'() ::
    #{status => boolean()}.

-type 'ShutdownRequest'() ::
    #{force => boolean()}.

-type 'ShutdownAck'() ::
    #{}.

-spec decoder() -> module().
%% The module (generated by gpb) used to encode and decode protobuf
%% messages.
decoder() -> distributed.

%% RPCs for service 'BenchmarkMaster'

-spec 'CheckIn'(Message::'ClientInfo'(), Stream::grpc:stream(), State::any()) ->
    {'CheckinResponse'(), grpc:stream()} | grpc:error_response().
%% This is a unary RPC
'CheckIn'(_Message, Stream, _State) ->
    {#{}, Stream}.

%% RPCs for service 'BenchmarkClient'

-spec 'Setup'(Message::'SetupConfig'(), Stream::grpc:stream(), State::any()) ->
    {'SetupResponse'(), grpc:stream()} | grpc:error_response().
%% This is a unary RPC
'Setup'(_Message, Stream, _State) ->
    {#{}, Stream}.

-spec 'Cleanup'(Message::'CleanupInfo'(), Stream::grpc:stream(), State::any()) ->
    {'CleanupResponse'(), grpc:stream()} | grpc:error_response().
%% This is a unary RPC
'Cleanup'(_Message, Stream, _State) ->
    {#{}, Stream}.

-spec 'Shutdown'(Message::'ShutdownRequest'(), Stream::grpc:stream(), State::any()) ->
    {'ShutdownAck'(), grpc:stream()} | grpc:error_response().
%% This is a unary RPC
'Shutdown'(_Message, Stream, _State) ->
    {#{}, Stream}.

