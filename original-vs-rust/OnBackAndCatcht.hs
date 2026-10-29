{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
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
  run "B1 undo: handlers in reverse order" $ do
    (say "a") `onUndo` (say "undo a")
    (say "b") `onUndo` (say "undo b")
    undo

  run "B2 retry resumes from the registration point" $ do
    n <- liftIO $ newMVar (0 :: Int)
    (say "start") `onUndo` (do say "undo"; retry)
    k <- liftIO $ modifyMVar n (\x -> return (x + 1, x + 1))
    if k == 1 then undo else say "done"

  run "B3 the handlers are inherited by later threads" $ do
    (return ()) `onUndo` (say "undo")
    abduce
    undo

  run "B4 a handler that stops cuts the backtracking" $ do
    (say "a") `onUndo` (say "undo a")
    (say "b") `onUndo` (do say "undo b"; empty)
    undo

  run "B5 localBack hides the handlers outside" $ do
    (return ()) `onUndo` (say "outer")
    localBack () $ do
       (return ()) `onUndo` (say "inner")
       undo

  run "B6 razon tipada" $ do
    onBack1 $ \(n :: Int) -> say ("reason " ++ show n)
    back (42 :: Int)

  run "B7 several threads do back: the handler stays registered for all" $ do
    onBack1 $ \(s :: String) -> say ("handler " ++ s)
    i <- choose [1,2,3 :: Int]
    back ("thread" ++ show i)

  run "B8 the same thread does back twice after retry" $ do
    n <- liftIO $ newMVar (0 :: Int)
    (return ()) `onUndo` (do say "undo"; retry)
    k <- liftIO $ modifyMVar n (\x -> return (x + 1, x + 1))
    say ("k=" ++ show k)
    when (k < 3) undo
    say "end"

  run "B9 backCut deletes the handlers" $ do
    (return ()) `onUndo` (say "old")
    undoCut
    (return ()) `onUndo` (say "new")
    undo

  run "B10 back without handlers" $ do
    say "before"
    undo
    say "not reached"

  run "B11 the handler returns a value with which the flow continues (forward)" $ do
    r <- (return "normal") `onBack` (\() -> do retry; return "from the handler")
    say r
    when (r == "normal") undo

  run "X6c catcht: exception in the first argument, not at the beginning" $ do
    r <- (do say "inside"; thr (ErrorCall "x"); return "no") `catcht` \(e :: ErrorCall) -> return ("recovered " ++ show e)
    say r

  run "X6d catcht: exception AFTER the first argument" $ do
    r <- (return "mx") `catcht` \(e :: ErrorCall) -> return ("recovered " ++ show e)
    say r
    thr (ErrorCall "after")

  run "X6e catcht with throwt inside the first argument" $ do
    r <- (do say "inside"; throwt (ErrorCall "t"); return "no") `catcht` \(e :: ErrorCall) -> return ("recovered " ++ show e)
    say r
