{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Data.Typeable
import Control.Applicative
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch)
import qualified System.Timeout as T

say :: String -> TransIO ()
say = liftIO . putStrLn

thr :: Exception e => e -> TransIO ()
thr = liftIO . throwIO

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 2000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' does not finish)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "X1 one onException; the exception in the code below" $ do
    onException $ \(e :: ErrorCall) -> say ("caught " ++ show e)
    say "before"
    thr (ErrorCall "boom")
    say "not reached"

  run "X2 several threads throw: the handler fires for each one" $ do
    onException $ \(e :: ErrorCall) -> say ("caught " ++ show e)
    i <- choose [1,2,3 :: Int]
    when (i /= 1) $ thr (ErrorCall (show i))
    say ("ok " ++ show i)

  run "X3 the filter by type: ErrorCall does not catch ArithException" $ do
    onException $ \(e :: ErrorCall) -> say ("ErrorCall " ++ show e)
    onException $ \(e :: ArithException) -> say ("Arith " ++ show e)
    thr DivideByZero

  run "X4 LIFO order of two handlers of the same type" $ do
    onException $ \(e :: ErrorCall) -> say "first registered"
    onException $ \(e :: ErrorCall) -> say "second registered"
    thr (ErrorCall "x")

  run "X5 continue: resumes forward from the registration point" $ do
    n <- liftIO $ newMVar (0 :: Int)
    onException $ \(e :: ErrorCall) -> do say "handler"; continue
    k <- liftIO $ modifyMVar n (\x -> return (x + 1, x + 1))
    say ("k=" ++ show k)
    when (k == 1) $ thr (ErrorCall "x")
    say "end"

  run "X6 catcht: the exception is in the first argument and the handler's value continues" $ do
    r <- (do liftIO (throwIO (ErrorCall "in mx")); return "no") `catcht` \(e :: ErrorCall) -> return ("recovered " ++ show e)
    say r

  run "X6b catcht without exception" $ do
    r <- return "normal" `catcht` \(e :: ErrorCall) -> return "no"
    say r

  run "X7 throwt keeps the state; throwIO does not" $ do
    setData "initial"
    onException $ \(e :: ErrorCall) -> do
      v <- getData
      say ("state: " ++ show (v :: Maybe String))
    setData "changed"
    throwt (ErrorCall "t")

  run "X7b with throwIO" $ do
    setData "initial"
    onException $ \(e :: ErrorCall) -> do
      v <- getData
      say ("state: " ++ show (v :: Maybe String))
    setData "changed"
    thr (ErrorCall "io")

  run "X8 exception in a thread launched after the onException" $ do
    onException $ \(e :: ErrorCall) -> say ("caught " ++ show e)
    abduce
    liftIO $ threadDelay 10000
    thr (ErrorCall "in thread")

  run "X9 no handler: keep' prints it" $ do
    thr (ErrorCall "no handler")
    say "not reached"

  run "X10 an onException registered AFTER the exception does not fire" $ do
    onException $ \(e :: ErrorCall) -> say "before"
    say "a"
    thr (ErrorCall "x") <|> return ()
    onException $ \(e :: ErrorCall) -> say "after (should not happen)"
    say "b"

  run "X11 localExceptions: the handlers inside are not valid outside" $ do
    onException $ \(e :: ErrorCall) -> say "outside"
    localExceptions $ do
      onException $ \(e :: ErrorCall) -> say "inside"
    thr (ErrorCall "x")

  run "X12 cutExceptions" $ do
    onException $ \(e :: ErrorCall) -> say "old"
    cutExceptions
    onException $ \(e :: ErrorCall) -> say "new"
    thr (ErrorCall "x")

  run "X13 a handler that stops (empty) cuts the chain" $ do
    onException $ \(e :: ErrorCall) -> say "first"
    onException $ \(e :: ErrorCall) -> do say "second and stop"; empty
    thr (ErrorCall "x")

  run "X14 SomeException atrapa todo" $ do
    onException $ \(e :: SomeException) -> say ("any: " ++ show e)
    thr DivideByZero

  run "X15 error puro (error)" $ do
    onException $ \(e :: ErrorCall) -> say ("caught " ++ show e)
    r <- liftIO $ evaluate (error "pure" :: Int)
    say (show r)
