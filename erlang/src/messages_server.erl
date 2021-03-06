-module(messages_server).

%% this file was generated by grpc

-export([decoder/0]).

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
decoder() -> messages.

